//! 缓存槽位结构定义

use core::sync::atomic::{AtomicU16, AtomicU64, AtomicU8, AtomicUsize, Ordering};

use crate::mm::PhysAddr;

/// 槽位状态标志
pub const SLOT_EMPTY: u8 = 0;
pub const SLOT_VALID: u8 = 1;
pub const SLOT_DIRTY: u8 = 2;
pub const SLOT_LOCKED: u8 = 4;

/// 无效的槽位索引
pub const INVALID_SLOT: u16 = u16::MAX;

/// 无效的页索引
pub const INVALID_PAGE_INDEX: u64 = u64::MAX;

/// 单个缓存槽位 - 紧凑设计
///
/// 每个槽位管理一个4KB的物理页帧，总共32字节元数据
#[repr(C)]
pub struct CacheSlot {
    /// 页索引 (字节偏移 / 4096)，INVALID_PAGE_INDEX表示空槽
    pub(super) page_index: AtomicU64,
    /// 物理页帧地址
    pub(super) phys_frame: AtomicUsize,
    /// 状态标志
    pub(super) flags: AtomicU8,
    /// LRU前驱槽位索引
    pub(super) lru_prev: AtomicU16,
    /// LRU后继槽位索引
    pub(super) lru_next: AtomicU16,
    /// 引用计数（用于并发访问）
    pub(super) ref_count: AtomicU8,
    /// 填充到32字节对齐
    _padding: [u8; 9],
}

impl CacheSlot {
    /// 创建一个空的缓存槽位
    pub const fn empty() -> Self {
        Self {
            page_index: AtomicU64::new(INVALID_PAGE_INDEX),
            phys_frame: AtomicUsize::new(0),
            flags: AtomicU8::new(SLOT_EMPTY),
            lru_prev: AtomicU16::new(INVALID_SLOT),
            lru_next: AtomicU16::new(INVALID_SLOT),
            ref_count: AtomicU8::new(0),
            _padding: [0; 9],
        }
    }

    /// 检查槽位是否为空
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.page_index.load(Ordering::Acquire) == INVALID_PAGE_INDEX
    }

    /// 检查槽位是否有效
    #[inline]
    pub fn is_valid(&self) -> bool {
        (self.flags.load(Ordering::Acquire) & SLOT_VALID) != 0
    }

    /// 检查槽位是否脏
    #[inline]
    pub fn is_dirty(&self) -> bool {
        (self.flags.load(Ordering::Acquire) & SLOT_DIRTY) != 0
    }

    /// 检查槽位是否被锁定
    #[inline]
    pub fn is_locked(&self) -> bool {
        (self.flags.load(Ordering::Acquire) & SLOT_LOCKED) != 0
    }

    /// 获取页索引
    #[inline]
    pub fn page_index(&self) -> u64 {
        self.page_index.load(Ordering::Acquire)
    }

    /// 获取物理页帧地址
    #[inline]
    pub fn phys_frame(&self) -> PhysAddr {
        PhysAddr::new(self.phys_frame.load(Ordering::Acquire))
    }

    /// 设置页索引
    #[inline]
    pub fn set_page_index(&self, index: u64) {
        self.page_index.store(index, Ordering::Release);
    }

    /// 设置物理页帧地址
    #[inline]
    pub fn set_phys_frame(&self, addr: PhysAddr) {
        self.phys_frame.store(addr.data(), Ordering::Release);
    }

    /// 设置为有效状态
    #[inline]
    pub fn set_valid(&self) {
        self.flags.fetch_or(SLOT_VALID, Ordering::Release);
    }

    /// 清除槽位（重置为空状态）
    pub fn clear(&self) {
        self.page_index.store(INVALID_PAGE_INDEX, Ordering::Release);
        self.flags.store(SLOT_EMPTY, Ordering::Release);
        self.ref_count.store(0, Ordering::Release);
    }
}

// 确保CacheSlot大小为32字节
const _: () = assert!(core::mem::size_of::<CacheSlot>() == 32);
