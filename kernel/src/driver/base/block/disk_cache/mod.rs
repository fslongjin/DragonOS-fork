//! 块设备磁盘缓存模块
//!
//! 以4KB Page为缓存单位，每个设备独立的缓存实例。
//! 元数据开销约0.8%（32字节管理4KB数据）。

pub mod slot;

use core::sync::atomic::{AtomicU16, Ordering};

use alloc::sync::{Arc, Weak};
use hashbrown::HashMap;
use system_error::SystemError;

use crate::{
    arch::MMArch,
    libs::rwlock::RwLock,
    mm::{allocator::page_frame::FrameAllocator, MemoryManagementArch, PhysAddr},
};

use self::slot::{CacheSlot, INVALID_PAGE_INDEX, INVALID_SLOT, SLOT_DIRTY};

use super::block_device::BlockDevice;

/// 缓存槽位数量（512个槽位 = 2MB缓存）
pub const CACHE_SLOTS: usize = 512;

/// 页大小
const PAGE_SIZE: usize = MMArch::PAGE_SIZE;

/// 磁盘缓存
///
/// 每个块设备独立的缓存实例，以4KB Page为缓存单位。
pub struct DiskCache {
    /// 弱引用设备
    device: Weak<dyn BlockDevice>,
    /// 缓存槽位数组（固定大小，一次性分配）
    slots: [CacheSlot; CACHE_SLOTS],
    /// 页索引 -> 槽位索引 映射
    index_map: RwLock<HashMap<u64, u16>>,
    /// LRU链表头（最近使用）
    lru_head: AtomicU16,
    /// LRU链表尾（最久未使用）
    lru_tail: AtomicU16,
    /// 已使用槽位数
    used_slots: AtomicU16,
}

impl DiskCache {
    /// 创建新的磁盘缓存
    ///
    /// 预分配所有物理页帧，避免运行时动态分配。
    pub fn new(device: Weak<dyn BlockDevice>) -> Result<Arc<Self>, SystemError> {
        // 使用 const 初始化数组
        const EMPTY_SLOT: CacheSlot = CacheSlot::empty();
        let mut slots = [EMPTY_SLOT; CACHE_SLOTS];

        // 预分配物理页帧
        for slot in slots.iter_mut() {
            let frame = unsafe {
                crate::arch::mm::LockedFrameAllocator
                    .allocate_one()
                    .ok_or(SystemError::ENOMEM)?
            };
            slot.set_phys_frame(PhysAddr::new(frame.data()));
        }

        Ok(Arc::new(Self {
            device,
            slots,
            index_map: RwLock::new(HashMap::new()),
            lru_head: AtomicU16::new(INVALID_SLOT),
            lru_tail: AtomicU16::new(INVALID_SLOT),
            used_slots: AtomicU16::new(0),
        }))
    }

    /// 从缓存读取数据
    ///
    /// 缓存命中时直接返回，未命中时自动提交BIO读取。
    ///
    /// ## 参数
    /// - `byte_offset`: 字节偏移量
    /// - `buf`: 目标缓冲区
    ///
    /// ## 返回值
    /// 成功读取的字节数
    pub fn read(&self, byte_offset: usize, buf: &mut [u8]) -> Result<usize, SystemError> {
        if buf.is_empty() {
            return Ok(0);
        }

        let page_start = byte_offset / PAGE_SIZE;
        let page_end = (byte_offset + buf.len() - 1) / PAGE_SIZE;

        let mut buf_offset = 0;

        for page_idx in page_start..=page_end {
            let page_byte_start = page_idx * PAGE_SIZE;

            // 计算本页内的偏移和长度
            let in_page_offset = if page_idx == page_start {
                byte_offset % PAGE_SIZE
            } else {
                0
            };

            let in_page_len = if page_idx == page_end {
                (byte_offset + buf.len() - 1) % PAGE_SIZE + 1 - in_page_offset
            } else {
                PAGE_SIZE - in_page_offset
            };

            // 尝试从缓存读取
            if let Some(slot_idx) = self.lookup_slot(page_idx as u64) {
                self.copy_from_slot(slot_idx, in_page_offset, &mut buf[buf_offset..buf_offset + in_page_len]);
                self.touch_lru(slot_idx);
            } else {
                // 缓存未命中，从设备读取
                self.read_page_from_device(page_idx as u64, page_byte_start)?;

                // 再次查找（现在应该在缓存中）
                if let Some(slot_idx) = self.lookup_slot(page_idx as u64) {
                    self.copy_from_slot(slot_idx, in_page_offset, &mut buf[buf_offset..buf_offset + in_page_len]);
                } else {
                    return Err(SystemError::EIO);
                }
            }

            buf_offset += in_page_len;
        }

        Ok(buf_offset)
    }

    /// 写入数据到缓存（Write-Through策略）
    ///
    /// 先写入设备，再更新缓存。
    ///
    /// ## 参数
    /// - `byte_offset`: 字节偏移量
    /// - `buf`: 源数据缓冲区
    ///
    /// ## 返回值
    /// 成功写入的字节数
    pub fn write(&self, byte_offset: usize, buf: &[u8]) -> Result<usize, SystemError> {
        if buf.is_empty() {
            return Ok(0);
        }

        // 先写入设备
        let device = self.device.upgrade().ok_or(SystemError::ENODEV)?;
        device.write_at_sync(byte_offset, buf.len(), buf)?;

        // 更新缓存中已存在的页
        let page_start = byte_offset / PAGE_SIZE;
        let page_end = (byte_offset + buf.len() - 1) / PAGE_SIZE;

        let mut buf_offset = 0;

        for page_idx in page_start..=page_end {
            let in_page_offset = if page_idx == page_start {
                byte_offset % PAGE_SIZE
            } else {
                0
            };

            let in_page_len = if page_idx == page_end {
                (byte_offset + buf.len() - 1) % PAGE_SIZE + 1 - in_page_offset
            } else {
                PAGE_SIZE - in_page_offset
            };

            // 如果页在缓存中，更新它
            if let Some(slot_idx) = self.lookup_slot(page_idx as u64) {
                self.copy_to_slot(slot_idx, in_page_offset, &buf[buf_offset..buf_offset + in_page_len]);
                self.touch_lru(slot_idx);
            }

            buf_offset += in_page_len;
        }

        Ok(buf.len())
    }

    /// 查找页索引对应的槽位
    fn lookup_slot(&self, page_idx: u64) -> Option<u16> {
        self.index_map.read().get(&page_idx).copied()
    }

    /// 从设备读取一页数据到缓存
    fn read_page_from_device(&self, page_idx: u64, byte_offset: usize) -> Result<(), SystemError> {
        let device = self.device.upgrade().ok_or(SystemError::ENODEV)?;

        // 分配或淘汰一个槽位
        let slot_idx = self.alloc_slot(page_idx);
        let slot = &self.slots[slot_idx as usize];

        // 读取数据到槽位的物理页帧
        let phys_addr = slot.phys_frame();
        let virt_addr = unsafe { MMArch::phys_2_virt(phys_addr) }.ok_or(SystemError::EFAULT)?;

        let page_buf = unsafe { core::slice::from_raw_parts_mut(virt_addr.data() as *mut u8, PAGE_SIZE) };

        device.read_at_sync(byte_offset, PAGE_SIZE, page_buf)?;

        // 标记为有效
        slot.set_page_index(page_idx);
        slot.set_valid();

        // 添加到索引映射
        self.index_map.write().insert(page_idx, slot_idx);

        Ok(())
    }

    /// 分配一个缓存槽位
    ///
    /// 优先使用空闲槽位，否则淘汰LRU尾部。
    fn alloc_slot(&self, page_idx: u64) -> u16 {
        let used = self.used_slots.load(Ordering::Acquire);

        if used < CACHE_SLOTS as u16 {
            // 使用空闲槽位
            let slot_idx = self.used_slots.fetch_add(1, Ordering::AcqRel);
            if slot_idx < CACHE_SLOTS as u16 {
                self.lru_push_front(slot_idx);
                return slot_idx;
            }
            // 竞争失败，回退到淘汰
            self.used_slots.fetch_sub(1, Ordering::AcqRel);
        }

        // 淘汰LRU尾部
        self.evict_lru_tail(page_idx)
    }

    /// 淘汰LRU尾部槽位
    fn evict_lru_tail(&self, new_page_idx: u64) -> u16 {
        let evict_idx = self.lru_tail.load(Ordering::Acquire);
        if evict_idx == INVALID_SLOT {
            // 不应该发生，但作为安全措施返回0
            return 0;
        }

        let slot = &self.slots[evict_idx as usize];

        // 从index_map中移除旧映射
        let old_page_idx = slot.page_index();
        if old_page_idx != INVALID_PAGE_INDEX {
            self.index_map.write().remove(&old_page_idx);
        }

        // 清除槽位状态
        slot.clear();
        slot.set_page_index(new_page_idx);

        // 移到LRU头部
        self.lru_move_to_front(evict_idx);

        evict_idx
    }

    /// 将槽位推入LRU头部
    fn lru_push_front(&self, slot_idx: u16) {
        let slot = &self.slots[slot_idx as usize];

        let old_head = self.lru_head.swap(slot_idx, Ordering::AcqRel);

        slot.lru_prev.store(INVALID_SLOT, Ordering::Release);
        slot.lru_next.store(old_head, Ordering::Release);

        if old_head != INVALID_SLOT {
            self.slots[old_head as usize].lru_prev.store(slot_idx, Ordering::Release);
        } else {
            // 链表为空，设置尾部
            self.lru_tail.store(slot_idx, Ordering::Release);
        }
    }

    /// 将槽位移到LRU头部
    fn lru_move_to_front(&self, slot_idx: u16) {
        let current_head = self.lru_head.load(Ordering::Acquire);
        if current_head == slot_idx {
            return; // 已经在头部
        }

        let slot = &self.slots[slot_idx as usize];
        let prev = slot.lru_prev.load(Ordering::Acquire);
        let next = slot.lru_next.load(Ordering::Acquire);

        // 从当前位置移除
        if prev != INVALID_SLOT {
            self.slots[prev as usize].lru_next.store(next, Ordering::Release);
        }
        if next != INVALID_SLOT {
            self.slots[next as usize].lru_prev.store(prev, Ordering::Release);
        }

        // 更新尾部（如果移除的是尾部）
        if self.lru_tail.load(Ordering::Acquire) == slot_idx {
            self.lru_tail.store(prev, Ordering::Release);
        }

        // 插入头部
        let old_head = self.lru_head.swap(slot_idx, Ordering::AcqRel);
        slot.lru_prev.store(INVALID_SLOT, Ordering::Release);
        slot.lru_next.store(old_head, Ordering::Release);

        if old_head != INVALID_SLOT {
            self.slots[old_head as usize].lru_prev.store(slot_idx, Ordering::Release);
        }
    }

    /// 触摸槽位（移到LRU头部）
    fn touch_lru(&self, slot_idx: u16) {
        self.lru_move_to_front(slot_idx);
    }

    /// 从槽位复制数据到缓冲区
    fn copy_from_slot(&self, slot_idx: u16, offset: usize, buf: &mut [u8]) {
        let slot = &self.slots[slot_idx as usize];
        let phys_addr = slot.phys_frame();

        if let Some(virt_addr) = unsafe { MMArch::phys_2_virt(phys_addr) } {
            let src = unsafe { core::slice::from_raw_parts(virt_addr.data() as *const u8, PAGE_SIZE) };
            buf.copy_from_slice(&src[offset..offset + buf.len()]);
        }
    }

    /// 复制数据到槽位
    fn copy_to_slot(&self, slot_idx: u16, offset: usize, buf: &[u8]) {
        let slot = &self.slots[slot_idx as usize];
        let phys_addr = slot.phys_frame();

        if let Some(virt_addr) = unsafe { MMArch::phys_2_virt(phys_addr) } {
            let dst = unsafe { core::slice::from_raw_parts_mut(virt_addr.data() as *mut u8, PAGE_SIZE) };
            dst[offset..offset + buf.len()].copy_from_slice(buf);
        }
    }

    /// 使缓存中的指定页失效
    #[allow(dead_code)]
    pub fn invalidate_page(&self, page_idx: u64) {
        if let Some(slot_idx) = self.index_map.write().remove(&page_idx) {
            self.slots[slot_idx as usize].clear();
        }
    }

    /// 同步所有脏页到设备
    #[allow(dead_code)]
    pub fn sync(&self) -> Result<(), SystemError> {
        let device = self.device.upgrade().ok_or(SystemError::ENODEV)?;

        for slot in self.slots.iter() {
            if slot.is_dirty() {
                let page_idx = slot.page_index();
                if page_idx == INVALID_PAGE_INDEX {
                    continue;
                }

                let phys_addr = slot.phys_frame();
                if let Some(virt_addr) = unsafe { MMArch::phys_2_virt(phys_addr) } {
                    let buf = unsafe { core::slice::from_raw_parts(virt_addr.data() as *const u8, PAGE_SIZE) };
                    let byte_offset = page_idx as usize * PAGE_SIZE;
                    device.write_at_sync(byte_offset, PAGE_SIZE, buf)?;

                    // 清除脏标志
                    slot.flags.fetch_and(!SLOT_DIRTY, Ordering::Release);
                }
            }
        }

        Ok(())
    }
}

impl Drop for DiskCache {
    fn drop(&mut self) {
        // 释放所有预分配的物理页帧
        for slot in self.slots.iter() {
            let phys_addr = slot.phys_frame();
            if phys_addr.data() != 0 {
                unsafe {
                    let _ = crate::arch::mm::LockedFrameAllocator.free_one(phys_addr);
                }
            }
        }
    }
}
