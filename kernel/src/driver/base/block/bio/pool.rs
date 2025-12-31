//! BioSegment 内存池
//!
//! 预分配连续物理页面，用于减少 Bio IO 操作时的内存分配开销。
//! 参考 Asterinas `kernel/comps/block/src/bio.rs:542-686`

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::arch::MMArch;
use crate::libs::spinlock::SpinLock;
use crate::mm::allocator::page_frame::{allocate_page_frames, PageFrameCount};
use crate::mm::page::Page;
use crate::mm::{MemoryManagementArch, PhysAddr};
use system_error::SystemError;

use super::segment::BioSegment;

/// 内存池槽位状态
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SlotState {
    /// 空闲
    Free,
    /// 已分配
    Allocated,
}

/// 内存池槽位管理器
struct PoolSlotManager {
    /// 槽位状态位图 (使用 Vec<bool> 简化实现，后续可优化为真正的位图)
    slots: Vec<SlotState>,
    /// 空闲槽位数
    free_count: usize,
}

impl PoolSlotManager {
    fn new(total_blocks: usize) -> Self {
        Self {
            slots: vec![SlotState::Free; total_blocks],
            free_count: total_blocks,
        }
    }

    /// 分配连续的 n 个槽位
    ///
    /// 使用首次适配算法
    fn allocate(&mut self, nblocks: usize) -> Option<usize> {
        if nblocks > self.free_count || nblocks == 0 {
            return None;
        }

        let mut start = 0;
        while start + nblocks <= self.slots.len() {
            // 检查从 start 开始的 nblocks 个槽位是否都空闲
            let all_free = self.slots[start..start + nblocks]
                .iter()
                .all(|&s| s == SlotState::Free);

            if all_free {
                // 标记为已分配
                for slot in &mut self.slots[start..start + nblocks] {
                    *slot = SlotState::Allocated;
                }
                self.free_count -= nblocks;
                return Some(start);
            }

            // 找到下一个空闲块开始搜索
            start += 1;
        }

        None
    }

    /// 释放从 start 开始的 nblocks 个槽位
    fn free(&mut self, start: usize, nblocks: usize) {
        if start + nblocks > self.slots.len() {
            log::error!(
                "BioSegmentPool: invalid free range: start={}, nblocks={}",
                start,
                nblocks
            );
            return;
        }

        for slot in &mut self.slots[start..start + nblocks] {
            *slot = SlotState::Free;
        }
        self.free_count += nblocks;
    }

    #[allow(dead_code)]
    fn free_count(&self) -> usize {
        self.free_count
    }
}

/// Bio 段内存池
///
/// 预分配一块连续的物理内存，用于 Bio IO 操作，
/// 减少频繁的小内存分配开销。
pub struct BioSegmentPool {
    /// 池的起始物理地址
    base_phys: PhysAddr,
    /// 总块数 (每块 PAGE_SIZE)
    total_blocks: usize,
    /// 槽位管理器
    manager: SpinLock<PoolSlotManager>,
    /// 分配计数器 (统计用)
    alloc_count: AtomicUsize,
    /// 释放计数器 (统计用)
    free_count: AtomicUsize,
}

impl BioSegmentPool {
    /// 默认池大小: 4MB (1024 * 4KB)
    pub const DEFAULT_BLOCKS: usize = 1024;

    /// 创建新的内存池
    ///
    /// # 参数
    /// - `nblocks`: 池的块数，每块大小为 PAGE_SIZE
    pub fn new(nblocks: usize) -> Result<Arc<Self>, SystemError> {
        let nblocks = if nblocks == 0 {
            Self::DEFAULT_BLOCKS
        } else {
            nblocks
        };

        // 分配连续物理页
        let (phys_addr, _) =
            unsafe { allocate_page_frames(PageFrameCount::new(nblocks)) }.ok_or_else(|| {
                log::error!("BioSegmentPool: failed to allocate {} pages", nblocks);
                SystemError::ENOMEM
            })?;

        log::info!(
            "BioSegmentPool: allocated {} pages at {:?}",
            nblocks,
            phys_addr
        );

        Ok(Arc::new(Self {
            base_phys: phys_addr,
            total_blocks: nblocks,
            manager: SpinLock::new(PoolSlotManager::new(nblocks)),
            alloc_count: AtomicUsize::new(0),
            free_count: AtomicUsize::new(0),
        }))
    }

    /// 从池中分配 BioSegment
    ///
    /// # 参数
    /// - `nblocks`: 需要的块数
    ///
    /// # 返回
    /// - `Some(BioSegment)`: 分配成功
    /// - `None`: 池空间不足
    pub fn alloc(&self, nblocks: usize) -> Option<PooledBioSegment> {
        let start = self.manager.lock().allocate(nblocks)?;

        self.alloc_count.fetch_add(1, Ordering::Relaxed);

        let offset = start * MMArch::PAGE_SIZE;
        let len = nblocks * MMArch::PAGE_SIZE;
        let phys_addr = PhysAddr::new(self.base_phys.data() + offset);

        Some(PooledBioSegment {
            phys_addr,
            len,
            pool_start: start,
            nblocks,
        })
    }

    /// 释放 BioSegment 到池
    fn free_segment(&self, start: usize, nblocks: usize) {
        self.manager.lock().free(start, nblocks);
        self.free_count.fetch_add(1, Ordering::Relaxed);
    }

    /// 获取总块数
    #[allow(dead_code)]
    pub fn total_blocks(&self) -> usize {
        self.total_blocks
    }

    /// 获取空闲块数
    #[allow(dead_code)]
    pub fn free_blocks(&self) -> usize {
        self.manager.lock().free_count()
    }

    /// 获取基地址
    #[allow(dead_code)]
    pub fn base_phys(&self) -> PhysAddr {
        self.base_phys
    }
}

/// 从池中分配的 BioSegment
///
/// 当 Drop 时自动归还到池中
pub struct PooledBioSegment {
    /// 物理地址
    phys_addr: PhysAddr,
    /// 长度（字节）
    len: usize,
    /// 在池中的起始槽位
    pool_start: usize,
    /// 占用的块数
    nblocks: usize,
}

impl PooledBioSegment {
    /// 获取物理地址
    pub fn phys_addr(&self) -> PhysAddr {
        self.phys_addr
    }

    /// 获取长度
    pub fn len(&self) -> usize {
        self.len
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 获取虚拟地址（通过物理地址转换）
    pub fn virt_addr(&self) -> Option<crate::mm::VirtAddr> {
        unsafe { MMArch::phys_2_virt(self.phys_addr) }
    }

    /// 读取数据到缓冲区
    pub fn read_to_buf(&self, buf: &mut [u8]) -> usize {
        let read_len = core::cmp::min(buf.len(), self.len);
        if let Some(vaddr) = self.virt_addr() {
            unsafe {
                let src = core::slice::from_raw_parts(vaddr.data() as *const u8, self.len);
                buf[..read_len].copy_from_slice(&src[..read_len]);
            }
        }
        read_len
    }

    /// 从缓冲区写入数据
    pub fn write_from_buf(&self, buf: &[u8]) -> usize {
        let write_len = core::cmp::min(buf.len(), self.len);
        if let Some(vaddr) = self.virt_addr() {
            unsafe {
                let dst = core::slice::from_raw_parts_mut(vaddr.data() as *mut u8, self.len);
                dst[..write_len].copy_from_slice(&buf[..write_len]);
            }
        }
        write_len
    }

    /// 转换为普通的 BioSegment（需要关联 Page）
    pub fn to_bio_segment(self, page: Arc<Page>) -> BioSegment {
        let offset = self.phys_addr.data() % MMArch::PAGE_SIZE;
        BioSegment::new(page, offset, self.len)
    }
}

// ============================================================================
// 全局内存池
// ============================================================================

use crate::libs::lazy_init::Lazy;

/// 全局读缓冲池
static READ_POOL: Lazy<Arc<BioSegmentPool>> = Lazy::new();

/// 全局写缓冲池
static WRITE_POOL: Lazy<Arc<BioSegmentPool>> = Lazy::new();

/// Bio 方向
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BioDirection {
    Read,
    Write,
}

/// 初始化全局 Bio 内存池
///
/// 应该在系统启动时调用一次
pub fn init_bio_pools() -> Result<(), SystemError> {
    if READ_POOL.initialized() {
        return Ok(());
    }

    let read_pool = BioSegmentPool::new(BioSegmentPool::DEFAULT_BLOCKS)?;
    let write_pool = BioSegmentPool::new(BioSegmentPool::DEFAULT_BLOCKS)?;

    READ_POOL.init(read_pool);
    WRITE_POOL.init(write_pool);

    log::info!("Bio segment pools initialized");
    Ok(())
}

/// 从全局池分配 BioSegment
///
/// # 参数
/// - `direction`: IO 方向
/// - `nblocks`: 需要的块数
pub fn alloc_from_pool(direction: BioDirection, nblocks: usize) -> Option<PooledBioSegment> {
    let pool = match direction {
        BioDirection::Read => READ_POOL.try_get()?,
        BioDirection::Write => WRITE_POOL.try_get()?,
    };

    pool.alloc(nblocks)
}

/// 获取全局读池引用
#[allow(dead_code)]
pub fn get_read_pool() -> Option<&'static Arc<BioSegmentPool>> {
    READ_POOL.try_get()
}

/// 获取全局写池引用
#[allow(dead_code)]
pub fn get_write_pool() -> Option<&'static Arc<BioSegmentPool>> {
    WRITE_POOL.try_get()
}

/// 检查全局池是否已初始化
pub fn pools_initialized() -> bool {
    READ_POOL.initialized() && WRITE_POOL.initialized()
}
