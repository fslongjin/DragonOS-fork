//! 块设备统一缓存层
//!
//! 提供基于 PageCache 的块设备缓存机制，替代原有的独立 BlockCache。
//! 每个块设备拥有自己的 PageCache，通过 BufferHead 实现块级粒度的缓存。

use alloc::sync::{Arc, Weak};
use core::fmt::Debug;
use core::sync::atomic::{AtomicBool, Ordering};
use hashbrown::HashMap;
use system_error::SystemError;

use crate::{
    arch::{mm::LockedFrameAllocator, MMArch},
    filesystem::page_cache::PageCache,
    libs::spinlock::SpinLock,
    mm::{
        buffer_head::{create_empty_buffers, BufferHeadIter},
        page::{
            page_manager_lock_irqsave, page_reclaimer_lock_irqsave, FileMapInfo, Page, PageFlags,
            PageType,
        },
        page_cache_ops::{PageCacheOperations, WritebackControl},
        MemoryManagementArch,
    },
};

use super::block_device::{BlockDevice, BlockId};

/// 全局块设备缓存管理器
static BLOCK_DEVICE_CACHES: SpinLock<Option<HashMap<usize, Arc<BlockDeviceCache>>>> =
    SpinLock::new(None);

/// 是否已完成缓存表初始化（避免每次都抢一次锁）。
static BLOCK_DEVICE_CACHES_INITED: AtomicBool = AtomicBool::new(false);

/// 初始化块设备缓存管理器
fn init_cache_map() {
    if BLOCK_DEVICE_CACHES_INITED.load(Ordering::Acquire) {
        return;
    }
    let mut guard = BLOCK_DEVICE_CACHES.lock();
    if guard.is_none() {
        *guard = Some(HashMap::new());
    }

    // 即使并发重复进入，也只会把 None -> Some 一次；这里标记为已初始化即可。
    BLOCK_DEVICE_CACHES_INITED.store(true, Ordering::Release);
}

/// 获取缓存映射（确保已初始化）
fn get_cache_map() -> &'static SpinLock<Option<HashMap<usize, Arc<BlockDeviceCache>>>> {
    init_cache_map();
    &BLOCK_DEVICE_CACHES
}

/// 块设备 PageCache 操作
///
/// 实现 PageCacheOperations 接口，用于 PageReclaimer 回写块设备脏页
#[derive(Debug)]
pub struct BlockDeviceCacheOps;

impl BlockDeviceCacheOps {
    /// 创建新的块设备 PageCache 操作
    pub fn new() -> Arc<Self> {
        Arc::new(Self)
    }
}

impl PageCacheOperations for BlockDeviceCacheOps {
    fn read_folio(&self, page: &Arc<Page>) -> Result<(), SystemError> {
        // 块设备页面的读取在 BlockDeviceCache::read_page_from_device 中完成
        // 这里只是标记页面为 uptodate
        page.write_irqsave().add_flags(PageFlags::PG_UPTODATE);
        Ok(())
    }

    fn writepages(&self, wbc: &mut WritebackControl) -> Result<(), SystemError> {
        // 实际的写回由 PageReclaimer::writeback_with_cache_ops 完成
        // 这个方法用于批量写回，目前暂不使用
        let _ = wbc;
        Ok(())
    }

    fn dirty_folio(&self, page: &Arc<Page>) -> bool {
        let mut guard = page.write_irqsave();

        // 标记所有 buffer heads 为脏
        if let Some(bh) = guard.buffers() {
            for buffer in BufferHeadIter::new(bh) {
                buffer.set_dirty();
            }
        }

        let was_dirty = guard.flags().contains(PageFlags::PG_DIRTY);
        if !was_dirty {
            guard.add_flags(PageFlags::PG_DIRTY);
        }
        !was_dirty
    }

    fn release_folio(&self, page: &Arc<Page>) -> bool {
        let guard = page.read_irqsave();

        // 如果有忙碌的 buffer heads，不能释放
        if let Some(bh) = guard.buffers() {
            for buffer in BufferHeadIter::new(bh) {
                if buffer.is_busy() {
                    return false;
                }
            }
        }

        true
    }

    fn is_partially_uptodate(&self, page: &Arc<Page>, from: usize, count: usize) -> bool {
        let guard = page.read_irqsave();

        if let Some(bh) = guard.buffers() {
            let to = from + count;

            for buffer in BufferHeadIter::new(bh) {
                let bh_start = buffer.data_offset();
                let bh_end = bh_start + buffer.size();

                if bh_end <= from {
                    continue;
                }
                if bh_start >= to {
                    break;
                }

                if !buffer.is_uptodate() {
                    return false;
                }
            }
            return true;
        }

        false
    }
}

/// 块设备缓存
///
/// 每个块设备拥有一个 BlockDeviceCache 实例，
/// 内部使用 PageCache 来缓存数据，通过 BufferHead 实现块级粒度。
#[derive(Debug)]
pub struct BlockDeviceCache {
    /// 块设备 ID（用于全局管理）
    #[allow(dead_code)]
    bdev_id: usize,
    /// 页面缓存
    page_cache: Arc<PageCache>,
    /// 块大小
    block_size: usize,
    /// 块设备的弱引用（用于 BufferHead 回写）
    bdev: Weak<dyn BlockDevice>,
}

impl BlockDeviceCache {
    /// 创建新的块设备缓存
    fn new(bdev_id: usize, block_size: usize, bdev: &Arc<dyn BlockDevice>) -> Arc<Self> {
        let page_cache = PageCache::new(None);

        // 设置 PageCacheOperations，使 PageReclaimer 能正确回写块设备脏页
        let cache_ops = BlockDeviceCacheOps::new();
        let _ = page_cache.set_cache_ops(cache_ops);

        Arc::new(Self {
            bdev_id,
            page_cache,
            block_size,
            bdev: Arc::downgrade(bdev),
        })
    }

    /// 获取或创建块设备的缓存
    ///
    /// # 参数
    /// - `bdev_id`: 块设备的唯一标识符
    /// - `block_size`: 块大小（字节）
    /// - `bdev`: 块设备的 Arc 引用
    pub fn get_or_create(
        bdev_id: usize,
        block_size: usize,
        bdev: &Arc<dyn BlockDevice>,
    ) -> Arc<Self> {
        // 快路径：只在锁内做 HashMap 查询。
        {
            let mut guard = get_cache_map().lock();
            let map = guard.as_mut().expect("Block device cache map not initialized");
            if let Some(cache) = map.get(&bdev_id) {
                return cache.clone();
            }
        }

        // 慢路径：在锁外构造 cache，避免持锁期间发生潜在的分配/初始化导致长时间占用或不可调度。
        let new_cache = Self::new(bdev_id, block_size, bdev);

        // 二次检查并插入。
        let mut guard = get_cache_map().lock();
        let map = guard.as_mut().expect("Block device cache map not initialized");
        if let Some(cache) = map.get(&bdev_id) {
            return cache.clone();
        }
        map.insert(bdev_id, new_cache.clone());
        new_cache
    }

    /// 移除块设备的缓存
    pub fn remove(bdev_id: usize) {
        let mut guard = get_cache_map().lock();
        if let Some(map) = guard.as_mut() {
            map.remove(&bdev_id);
        }
    }

    /// 计算每页包含的块数
    #[inline]
    fn blocks_per_page(&self) -> usize {
        MMArch::PAGE_SIZE / self.block_size
    }

    /// 将块号转换为页索引
    #[inline]
    fn block_to_page_index(&self, block_nr: BlockId) -> usize {
        block_nr / self.blocks_per_page()
    }

    /// 将块号转换为页内偏移
    #[inline]
    fn block_to_page_offset(&self, block_nr: BlockId) -> usize {
        (block_nr % self.blocks_per_page()) * self.block_size
    }

    /// 从缓存读取数据
    ///
    /// # 参数
    /// - `bdev`: 块设备引用（用于缓存未命中时读取数据）
    /// - `lba_start`: 起始块号
    /// - `count`: 块数量
    /// - `buf`: 目标缓冲区
    pub fn read<B: BlockDevice + ?Sized>(
        &self,
        bdev: &B,
        lba_start: BlockId,
        count: usize,
        buf: &mut [u8],
    ) -> Result<usize, SystemError> {
        let mut bytes_read = 0;
        let mut current_block = lba_start;

        while bytes_read < buf.len() && current_block < lba_start + count {
            let page_index = self.block_to_page_index(current_block);
            let page_offset = self.block_to_page_offset(current_block);

            // 获取或创建页面
            let page = self.get_or_create_page(page_index)?;

            // 确保页面数据是最新的
            if !self.is_page_uptodate(&page) {
                self.read_page_from_device(&page, page_index, bdev)?;
            }

            // 计算本次要读取的数据量
            let page_remaining = MMArch::PAGE_SIZE - page_offset;
            let buf_remaining = buf.len() - bytes_read;
            let blocks_remaining = (lba_start + count - current_block) * self.block_size;
            let to_copy = page_remaining.min(buf_remaining).min(blocks_remaining);

            // 从页面复制数据到缓冲区
            let guard = page.read_irqsave();
            let page_data = unsafe { guard.as_slice() };
            buf[bytes_read..bytes_read + to_copy]
                .copy_from_slice(&page_data[page_offset..page_offset + to_copy]);

            bytes_read += to_copy;
            current_block += (to_copy + self.block_size - 1) / self.block_size;
        }

        Ok(bytes_read)
    }

    /// 向缓存写入数据
    ///
    /// # 参数
    /// - `bdev`: 块设备引用（用于读取部分页或写回）
    /// - `lba_start`: 起始块号
    /// - `count`: 块数量
    /// - `buf`: 源数据缓冲区
    pub fn write<B: BlockDevice + ?Sized>(
        &self,
        bdev: &B,
        lba_start: BlockId,
        count: usize,
        buf: &[u8],
    ) -> Result<usize, SystemError> {
        let mut bytes_written = 0;
        let mut current_block = lba_start;

        while bytes_written < buf.len() && current_block < lba_start + count {
            let page_index = self.block_to_page_index(current_block);
            let page_offset = self.block_to_page_offset(current_block);

            // 获取或创建页面
            let page = self.get_or_create_page(page_index)?;

            // 如果不是写整页，需要先读取页面内容
            let write_start = page_offset;
            let buf_remaining = buf.len() - bytes_written;
            let blocks_remaining = (lba_start + count - current_block) * self.block_size;
            let to_copy = (MMArch::PAGE_SIZE - page_offset)
                .min(buf_remaining)
                .min(blocks_remaining);
            let write_end = write_start + to_copy;

            // 如果不是覆盖整页，先确保页面有数据
            if write_start > 0 || write_end < MMArch::PAGE_SIZE {
                if !self.is_page_uptodate(&page) {
                    self.read_page_from_device(&page, page_index, bdev)?;
                }
            }

            // 写入数据到页面
            {
                let mut guard = page.write_irqsave();
                let page_data = unsafe { guard.as_slice_mut() };
                page_data[write_start..write_end]
                    .copy_from_slice(&buf[bytes_written..bytes_written + to_copy]);

                // 标记页面为脏
                guard.add_flags(PageFlags::PG_DIRTY | PageFlags::PG_UPTODATE);

                // 标记相关的 buffer heads 为脏
                if let Some(bh) = guard.buffers() {
                    let start_bh_idx = write_start / self.block_size;
                    let end_bh_idx = (write_end + self.block_size - 1) / self.block_size;

                    let mut bh_idx = 0;
                    for buffer in BufferHeadIter::new(bh) {
                        if bh_idx >= start_bh_idx && bh_idx < end_bh_idx {
                            buffer.set_dirty();
                            buffer.set_uptodate();
                        }
                        bh_idx += 1;
                        if bh_idx >= self.blocks_per_page() {
                            break;
                        }
                    }
                }
            }

            // 延迟写入模式：不立即写回设备，由 PageReclaimer 或 sync 操作负责
            // 如果需要 write-through 模式，取消下面注释
            // self.write_page_to_device(&page, page_index, bdev)?;

            bytes_written += to_copy;
            current_block += (to_copy + self.block_size - 1) / self.block_size;
        }

        Ok(bytes_written)
    }

    /// 获取或创建指定索引的页面
    fn get_or_create_page(&self, page_index: usize) -> Result<Arc<Page>, SystemError> {
        // 先尝试从缓存获取
        {
            let cache_guard = self.page_cache.lock_irqsave();
            if let Some(page) = cache_guard.get_page(page_index) {
                return Ok(page);
            }
        }

        // 创建新页面（在 PAGE_MANAGER 锁内）
        let page = {
            let mut page_manager = page_manager_lock_irqsave();
            page_manager.create_one_page(
                PageType::File(FileMapInfo {
                    page_cache: Arc::downgrade(&self.page_cache),
                    index: page_index,
                }),
                PageFlags::PG_LRU,
                &mut LockedFrameAllocator,
            )?
        };
        // PAGE_MANAGER 锁已释放

        // 将页面添加到 LRU（在 PAGE_MANAGER 锁外，避免锁顺序问题）
        {
            let mut reclaimer = page_reclaimer_lock_irqsave();
            reclaimer.insert_page(page.phys_address(), &page);
        }

        // 创建 buffer heads
        let bh = create_empty_buffers(&page, self.block_size)?;

        // 设置 buffer heads 的块映射
        {
            let mut guard = page.write_irqsave();
            guard.attach_buffers(bh.clone());
        }

        // 获取块设备的 Arc 引用（用于 map_to_block）
        let bdev = self.bdev.upgrade().ok_or(SystemError::ENODEV)?;

        // 为每个 buffer head 设置块号和块设备
        let base_block = page_index * self.blocks_per_page();
        let mut block_idx = 0;
        for buffer in BufferHeadIter::new(bh) {
            let block_nr = (base_block + block_idx) as u64;
            // 使用 map_to_block 同时设置 bdev、blocknr 和 mapped 标志
            buffer.map_to_block(&bdev, block_nr);
            block_idx += 1;
            if block_idx >= self.blocks_per_page() {
                break;
            }
        }

        // 添加到缓存
        self.page_cache.lock_irqsave().add_page(page_index, &page);

        Ok(page)
    }

    /// 检查页面是否是最新的
    fn is_page_uptodate(&self, page: &Arc<Page>) -> bool {
        page.read_irqsave().flags().contains(PageFlags::PG_UPTODATE)
    }

    /// 从设备读取页面数据
    fn read_page_from_device<B: BlockDevice + ?Sized>(
        &self,
        page: &Arc<Page>,
        page_index: usize,
        bdev: &B,
    ) -> Result<(), SystemError> {
        let base_block = page_index * self.blocks_per_page();

        // 读取整个页面的数据
        let mut guard = page.write_irqsave();
        let data = unsafe { guard.as_slice_mut() };

        bdev.read_at_sync(base_block, self.blocks_per_page(), data)?;

        // 标记页面和所有 buffer heads 为 uptodate
        guard.add_flags(PageFlags::PG_UPTODATE);

        if let Some(bh) = guard.buffers() {
            for buffer in BufferHeadIter::new(bh) {
                buffer.set_uptodate();
            }
        }

        Ok(())
    }

    /// 将页面数据写入设备
    fn write_page_to_device<B: BlockDevice + ?Sized>(
        &self,
        page: &Arc<Page>,
        page_index: usize,
        bdev: &B,
    ) -> Result<(), SystemError> {
        let base_block = page_index * self.blocks_per_page();

        let guard = page.read_irqsave();

        // 检查是否有脏的 buffer heads
        let mut has_dirty = false;
        if let Some(bh) = guard.buffers() {
            for buffer in BufferHeadIter::new(bh.clone()) {
                if buffer.is_dirty() {
                    has_dirty = true;
                    break;
                }
            }
        }

        if !has_dirty && !guard.flags().contains(PageFlags::PG_DIRTY) {
            return Ok(());
        }

        // 写入整个页面
        let data = unsafe { guard.as_slice() };
        bdev.write_at_sync(base_block, self.blocks_per_page(), data)?;

        // 清除脏标志
        drop(guard);
        let mut guard = page.write_irqsave();
        guard.remove_flags(PageFlags::PG_DIRTY);

        if let Some(bh) = guard.buffers() {
            for buffer in BufferHeadIter::new(bh) {
                buffer.clear_dirty();
            }
        }

        Ok(())
    }

    /// 同步所有脏页到设备
    ///
    /// # 参数
    /// - `bdev`: 块设备引用
    pub fn sync<B: BlockDevice + ?Sized>(&self, bdev: &B) -> Result<(), SystemError> {
        // 遍历所有页面，写回脏页
        let pages: alloc::vec::Vec<(usize, Arc<Page>)> = {
            let cache_guard = self.page_cache.lock_irqsave();
            cache_guard
                .pages()
                .iter()
                .map(|(idx, page)| (*idx, page.clone()))
                .collect()
        };

        for (page_index, page) in pages {
            let guard = page.read_irqsave();
            if guard.flags().contains(PageFlags::PG_DIRTY) {
                drop(guard);
                self.write_page_to_device(&page, page_index, bdev)?;
            }
        }

        Ok(())
    }

    /// 获取缓存的页面缓存引用（用于 PageReclaimer）
    #[allow(dead_code)]
    pub fn page_cache(&self) -> &Arc<PageCache> {
        &self.page_cache
    }

    /// 获取块大小
    #[inline]
    #[allow(dead_code)]
    pub fn block_size(&self) -> usize {
        self.block_size
    }
}
