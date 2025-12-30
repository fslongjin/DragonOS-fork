//! Buffer Head 实现
//!
//! BufferHead 是 Page 和 Block 之间的桥梁。
//! 当块大小小于页大小时，一个 Page 可以包含多个 BufferHead（形成环形链表）。
//!
//! 参考 Linux 6.6.21 的 buffer_head 实现

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use alloc::sync::{Arc, Weak};
use system_error::SystemError;

use crate::{
    driver::base::block::block_device::BlockDevice,
    libs::spinlock::SpinLock,
};

use super::page::Page;

bitflags! {
    /// Buffer Head 状态标志
    pub struct BhState: u32 {
        /// 包含有效数据
        const BH_UPTODATE = 1 << 0;
        /// 已修改，需要写回
        const BH_DIRTY = 1 << 1;
        /// 被锁定（I/O 进行中）
        const BH_LOCK = 1 << 2;
        /// 有磁盘映射
        const BH_MAPPED = 1 << 3;
        /// 新创建的块映射
        const BH_NEW = 1 << 4;
        /// 异步读取进行中
        const BH_ASYNC_READ = 1 << 5;
        /// 异步写入进行中
        const BH_ASYNC_WRITE = 1 << 6;
        /// 延迟分配（尚未在磁盘上分配）
        const BH_DELAY = 1 << 7;
        /// 块后面有不连续
        const BH_BOUNDARY = 1 << 8;
        /// 写入时发生 I/O 错误
        const BH_WRITE_EIO = 1 << 9;
        /// 已分配但未写入
        const BH_UNWRITTEN = 1 << 10;
    }
}

/// BufferHead 桥接 Page 和 Block
///
/// 一个 Page 可以有多个 BufferHead（当 block_size < PAGE_SIZE 时）。
/// BufferHead 通过 `this_page` 字段形成环形链表。
#[derive(Debug)]
pub struct BufferHead {
    /// Buffer 状态标志
    state: AtomicU32,

    /// 同一页面内的下一个 buffer（环形链表）
    this_page: SpinLock<Option<Arc<BufferHead>>>,

    /// 指向所属页面的弱引用
    page: Weak<Page>,

    /// 块设备上的逻辑块号
    blocknr: SpinLock<u64>,

    /// 块大小（字节）
    size: usize,

    /// 在页内的数据偏移量
    data_offset: usize,

    /// 块设备的弱引用
    bdev: SpinLock<Option<Weak<dyn BlockDevice>>>,

    /// 引用计数
    count: AtomicUsize,

    /// 用于同步 I/O 完成状态的锁
    uptodate_lock: SpinLock<()>,
}

impl BufferHead {
    /// 创建新的 BufferHead
    ///
    /// # 参数
    /// - `page`: 所属页面
    /// - `offset`: 在页内的偏移量
    /// - `size`: 块大小
    pub fn new(page: &Arc<Page>, offset: usize, size: usize) -> Arc<Self> {
        Arc::new(Self {
            state: AtomicU32::new(0),
            this_page: SpinLock::new(None),
            page: Arc::downgrade(page),
            blocknr: SpinLock::new(0),
            size,
            data_offset: offset,
            bdev: SpinLock::new(None),
            count: AtomicUsize::new(1),
            uptodate_lock: SpinLock::new(()),
        })
    }

    /// 获取块大小
    #[inline]
    pub fn size(&self) -> usize {
        self.size
    }

    /// 获取在页内的偏移量
    #[inline]
    pub fn data_offset(&self) -> usize {
        self.data_offset
    }

    /// 获取块号
    #[inline]
    pub fn blocknr(&self) -> u64 {
        *self.blocknr.lock()
    }

    /// 设置块号
    #[inline]
    pub fn set_blocknr(&self, blocknr: u64) {
        *self.blocknr.lock() = blocknr;
    }

    /// 获取所属页面
    pub fn page(&self) -> Option<Arc<Page>> {
        self.page.upgrade()
    }

    /// 获取块设备
    pub fn bdev(&self) -> Option<Arc<dyn BlockDevice>> {
        self.bdev.lock().as_ref().and_then(|w| w.upgrade())
    }

    /// 设置块设备
    pub fn set_bdev(&self, bdev: Option<Weak<dyn BlockDevice>>) {
        *self.bdev.lock() = bdev;
    }

    /// 将此 buffer 映射到指定的块
    pub fn map_to_block(&self, bdev: &Arc<dyn BlockDevice>, blocknr: u64) {
        *self.bdev.lock() = Some(Arc::downgrade(bdev));
        *self.blocknr.lock() = blocknr;
        self.set_mapped();
    }

    /// 获取下一个 buffer（环形链表中的下一个）
    pub fn next(&self) -> Option<Arc<BufferHead>> {
        self.this_page.lock().clone()
    }

    /// 设置下一个 buffer
    pub fn set_next(&self, next: Option<Arc<BufferHead>>) {
        *self.this_page.lock() = next;
    }

    // ==================== 状态操作 ====================

    fn get_state(&self) -> BhState {
        BhState::from_bits_truncate(self.state.load(Ordering::Acquire))
    }

    fn set_state_bit(&self, bit: BhState) {
        self.state.fetch_or(bit.bits(), Ordering::Release);
    }

    fn clear_state_bit(&self, bit: BhState) {
        self.state.fetch_and(!bit.bits(), Ordering::Release);
    }

    fn test_state_bit(&self, bit: BhState) -> bool {
        self.get_state().contains(bit)
    }

    /// 设置 uptodate 标志
    #[inline]
    pub fn set_uptodate(&self) {
        self.set_state_bit(BhState::BH_UPTODATE);
    }

    /// 清除 uptodate 标志
    #[inline]
    pub fn clear_uptodate(&self) {
        self.clear_state_bit(BhState::BH_UPTODATE);
    }

    /// 检查是否 uptodate
    #[inline]
    pub fn is_uptodate(&self) -> bool {
        self.test_state_bit(BhState::BH_UPTODATE)
    }

    /// 设置 dirty 标志
    #[inline]
    pub fn set_dirty(&self) {
        self.set_state_bit(BhState::BH_DIRTY);
    }

    /// 清除 dirty 标志
    #[inline]
    pub fn clear_dirty(&self) {
        self.clear_state_bit(BhState::BH_DIRTY);
    }

    /// 检查是否 dirty
    #[inline]
    pub fn is_dirty(&self) -> bool {
        self.test_state_bit(BhState::BH_DIRTY)
    }

    /// 测试并清除 dirty 标志（原子操作）
    pub fn test_clear_dirty(&self) -> bool {
        let old = self.state.fetch_and(!BhState::BH_DIRTY.bits(), Ordering::AcqRel);
        BhState::from_bits_truncate(old).contains(BhState::BH_DIRTY)
    }

    /// 设置 mapped 标志
    #[inline]
    pub fn set_mapped(&self) {
        self.set_state_bit(BhState::BH_MAPPED);
    }

    /// 清除 mapped 标志
    #[inline]
    pub fn clear_mapped(&self) {
        self.clear_state_bit(BhState::BH_MAPPED);
    }

    /// 检查是否 mapped
    #[inline]
    pub fn is_mapped(&self) -> bool {
        self.test_state_bit(BhState::BH_MAPPED)
    }

    /// 设置 lock 标志
    #[inline]
    pub fn set_lock(&self) {
        self.set_state_bit(BhState::BH_LOCK);
    }

    /// 清除 lock 标志
    #[inline]
    pub fn clear_lock(&self) {
        self.clear_state_bit(BhState::BH_LOCK);
    }

    /// 检查是否 locked
    #[inline]
    pub fn is_locked(&self) -> bool {
        self.test_state_bit(BhState::BH_LOCK)
    }

    /// 尝试锁定 buffer
    pub fn try_lock(&self) -> bool {
        let old = self.state.fetch_or(BhState::BH_LOCK.bits(), Ordering::AcqRel);
        !BhState::from_bits_truncate(old).contains(BhState::BH_LOCK)
    }

    /// 解锁 buffer
    pub fn unlock(&self) {
        self.clear_lock();
    }

    /// 设置 new 标志
    #[inline]
    pub fn set_new(&self) {
        self.set_state_bit(BhState::BH_NEW);
    }

    /// 清除 new 标志
    #[inline]
    pub fn clear_new(&self) {
        self.clear_state_bit(BhState::BH_NEW);
    }

    /// 检查是否 new
    #[inline]
    pub fn is_new(&self) -> bool {
        self.test_state_bit(BhState::BH_NEW)
    }

    /// 设置 async_read 标志
    #[inline]
    pub fn set_async_read(&self) {
        self.set_state_bit(BhState::BH_ASYNC_READ);
    }

    /// 清除 async_read 标志
    #[inline]
    pub fn clear_async_read(&self) {
        self.clear_state_bit(BhState::BH_ASYNC_READ);
    }

    /// 检查是否 async_read
    #[inline]
    pub fn is_async_read(&self) -> bool {
        self.test_state_bit(BhState::BH_ASYNC_READ)
    }

    /// 设置 async_write 标志
    #[inline]
    pub fn set_async_write(&self) {
        self.set_state_bit(BhState::BH_ASYNC_WRITE);
    }

    /// 清除 async_write 标志
    #[inline]
    pub fn clear_async_write(&self) {
        self.clear_state_bit(BhState::BH_ASYNC_WRITE);
    }

    /// 检查是否 async_write
    #[inline]
    pub fn is_async_write(&self) -> bool {
        self.test_state_bit(BhState::BH_ASYNC_WRITE)
    }

    /// 设置 delay 标志
    #[inline]
    pub fn set_delay(&self) {
        self.set_state_bit(BhState::BH_DELAY);
    }

    /// 清除 delay 标志
    #[inline]
    pub fn clear_delay(&self) {
        self.clear_state_bit(BhState::BH_DELAY);
    }

    /// 检查是否 delay
    #[inline]
    pub fn is_delay(&self) -> bool {
        self.test_state_bit(BhState::BH_DELAY)
    }

    /// 设置 write_eio 标志
    #[inline]
    pub fn set_write_eio(&self) {
        self.set_state_bit(BhState::BH_WRITE_EIO);
    }

    /// 清除 write_eio 标志
    #[inline]
    pub fn clear_write_eio(&self) {
        self.clear_state_bit(BhState::BH_WRITE_EIO);
    }

    /// 检查是否有 write_eio
    #[inline]
    pub fn has_write_eio(&self) -> bool {
        self.test_state_bit(BhState::BH_WRITE_EIO)
    }

    // ==================== 引用计数 ====================

    /// 增加引用计数
    pub fn get(&self) {
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    /// 减少引用计数
    pub fn put(&self) {
        self.count.fetch_sub(1, Ordering::Relaxed);
    }

    /// 获取当前引用计数
    pub fn ref_count(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    // ==================== 同步操作 ====================

    /// 获取 uptodate_lock
    pub fn uptodate_lock(&self) -> &SpinLock<()> {
        &self.uptodate_lock
    }

    /// 检查 buffer 是否繁忙（被锁定或脏）
    pub fn is_busy(&self) -> bool {
        let state = self.get_state();
        self.ref_count() > 1
            || state.contains(BhState::BH_DIRTY)
            || state.contains(BhState::BH_LOCK)
    }
}

/// 为页面创建空的 buffer heads 环形链表
///
/// # 参数
/// - `page`: 目标页面
/// - `block_size`: 块大小
///
/// # 返回
/// 第一个 BufferHead（链表头）
pub fn create_empty_buffers(page: &Arc<Page>, block_size: usize) -> Result<Arc<BufferHead>, SystemError> {
    use crate::arch::MMArch;
    use crate::mm::MemoryManagementArch;

    if block_size == 0 || block_size > MMArch::PAGE_SIZE {
        return Err(SystemError::EINVAL);
    }

    if MMArch::PAGE_SIZE % block_size != 0 {
        return Err(SystemError::EINVAL);
    }

    let blocks_per_page = MMArch::PAGE_SIZE / block_size;

    // 创建所有 buffer heads
    let mut buffers: alloc::vec::Vec<Arc<BufferHead>> = alloc::vec::Vec::with_capacity(blocks_per_page);
    for i in 0..blocks_per_page {
        let offset = i * block_size;
        let bh = BufferHead::new(page, offset, block_size);
        buffers.push(bh);
    }

    // 链接成环形链表
    for i in 0..blocks_per_page {
        let next_idx = (i + 1) % blocks_per_page;
        buffers[i].set_next(Some(buffers[next_idx].clone()));
    }

    Ok(buffers[0].clone())
}

/// 遍历页面的所有 buffer heads
pub struct BufferHeadIter {
    first: Arc<BufferHead>,
    current: Option<Arc<BufferHead>>,
    started: bool,
}

impl BufferHeadIter {
    pub fn new(head: Arc<BufferHead>) -> Self {
        Self {
            first: head.clone(),
            current: Some(head),
            started: false,
        }
    }
}

impl Iterator for BufferHeadIter {
    type Item = Arc<BufferHead>;

    fn next(&mut self) -> Option<Self::Item> {
        if !self.started {
            self.started = true;
            return self.current.clone();
        }

        if let Some(ref current) = self.current {
            let next = current.next();
            if let Some(ref next_bh) = next {
                // 检查是否回到了起点
                if Arc::ptr_eq(next_bh, &self.first) {
                    self.current = None;
                    return None;
                }
            }
            self.current = next;
            return self.current.clone();
        }

        None
    }
}
