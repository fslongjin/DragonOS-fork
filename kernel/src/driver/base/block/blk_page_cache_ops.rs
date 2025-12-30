//! 块设备 PageCache 操作实现
//!
//! 为块设备提供 PageCacheOperations 的实现，
//! 使块设备可以使用统一的 PageCache 机制进行缓存。
//!
//! 注意：本模块的代码目前未被使用，保留用于未来的 PageReclaimer 集成。
//! 当前的块设备缓存功能由 `block_dev_cache` 模块实现。

#![allow(dead_code)]

use alloc::sync::{Arc, Weak};
use system_error::SystemError;

use crate::{
    arch::MMArch,
    mm::{
        buffer_head::{create_empty_buffers, BufferHeadIter},
        page::{Page, PageFlags, PageType},
        page_cache_ops::{PageCacheOperations, WritebackControl},
        MemoryManagementArch,
    },
};

use super::block_device::BlockDevice;

/// 块设备的 PageCache 操作
///
/// 实现了通过 BufferHead 机制将页面与块设备关联的操作。
#[derive(Debug)]
pub struct BlockDevicePageCacheOps {
    /// 块设备的弱引用
    bdev: Weak<dyn BlockDevice>,
    /// 块大小
    block_size: usize,
}

impl BlockDevicePageCacheOps {
    /// 创建新的块设备 PageCache 操作
    pub fn new(bdev: &Arc<dyn BlockDevice>) -> Self {
        Self {
            bdev: Arc::downgrade(bdev),
            block_size: bdev.block_size(),
        }
    }

    /// 获取块设备
    fn bdev(&self) -> Result<Arc<dyn BlockDevice>, SystemError> {
        self.bdev.upgrade().ok_or(SystemError::EIO)
    }

    /// 计算每页包含的块数
    fn blocks_per_page(&self) -> usize {
        MMArch::PAGE_SIZE / self.block_size
    }
}

impl PageCacheOperations for BlockDevicePageCacheOps {
    /// 从块设备读取页面数据
    ///
    /// 1. 如果页面没有 buffer heads，创建它们
    /// 2. 为每个 buffer head 设置块映射
    /// 3. 从块设备读取数据到页面
    /// 4. 标记所有 buffers 和页面为 uptodate
    fn read_folio(&self, page: &Arc<Page>) -> Result<(), SystemError> {
        let bdev = self.bdev()?;

        // 获取页面索引
        let page_index = {
            let guard = page.read_irqsave();
            match guard.page_type() {
                PageType::File(info) => info.index,
                _ => return Err(SystemError::EINVAL),
            }
        };

        // 确保页面有 buffer heads
        let head_bh = {
            let mut guard = page.write_irqsave();
            if !guard.has_buffers() {
                let bh = create_empty_buffers(page, self.block_size)?;
                guard.attach_buffers(bh.clone());
                bh
            } else {
                guard.buffers().unwrap()
            }
        };

        // 计算起始块号
        let blocks_per_page = self.blocks_per_page();
        let start_block = page_index * blocks_per_page;

        // 读取每个 buffer
        let mut block_idx = 0;
        for bh in BufferHeadIter::new(head_bh) {
            if block_idx >= blocks_per_page {
                break;
            }

            let block_nr = (start_block + block_idx) as u64;

            // 映射 buffer 到块
            bh.map_to_block(&bdev, block_nr);

            // 从块设备读取数据
            let mut guard = page.write_irqsave();
            let data = unsafe {
                let slice = guard.as_slice_mut();
                &mut slice[bh.data_offset()..bh.data_offset() + bh.size()]
            };

            bdev.read_at_sync(block_nr as usize, 1, data)?;

            // 标记 buffer 为 uptodate
            bh.set_uptodate();

            block_idx += 1;
        }

        // 标记页面为 uptodate
        page.write_irqsave().add_flags(PageFlags::PG_UPTODATE);

        Ok(())
    }

    /// 将脏页写回块设备
    fn writepages(&self, wbc: &mut WritebackControl) -> Result<(), SystemError> {
        // 这个方法通常由 PageCache 层调用来遍历脏页
        // 在当前简化实现中，实际的写回由 dirty_folio 的页面单独处理
        // TODO: 实现完整的批量写回
        let _ = wbc;
        Ok(())
    }

    /// 标记页面为脏
    ///
    /// 同时标记所有关联的 buffer heads 为脏
    fn dirty_folio(&self, page: &Arc<Page>) -> bool {
        let mut guard = page.write_irqsave();

        // 标记所有 buffer heads 为脏
        if let Some(bh) = guard.buffers() {
            for buffer in BufferHeadIter::new(bh) {
                buffer.set_dirty();
            }
        }

        // 检查页面是否已经是脏的
        let was_dirty = guard.flags().contains(PageFlags::PG_DIRTY);
        if !was_dirty {
            guard.add_flags(PageFlags::PG_DIRTY);
        }
        !was_dirty
    }

    /// 释放页面
    ///
    /// 检查所有 buffer heads 是否可以释放
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

    /// 检查页面是否部分有效
    fn is_partially_uptodate(&self, page: &Arc<Page>, from: usize, count: usize) -> bool {
        let guard = page.read_irqsave();

        if let Some(bh) = guard.buffers() {
            let to = from + count;

            for buffer in BufferHeadIter::new(bh) {
                let bh_start = buffer.data_offset();
                let bh_end = bh_start + buffer.size();

                // 检查是否与请求范围重叠
                if bh_end <= from {
                    continue;
                }
                if bh_start >= to {
                    break;
                }

                // 如果重叠但不是 uptodate，返回 false
                if !buffer.is_uptodate() {
                    return false;
                }
            }
            return true;
        }

        false
    }
}

/// 将脏页写回块设备
///
/// 这是一个辅助函数，用于将单个脏页写回块设备。
/// 遍历页面的所有 buffer heads，将脏的 buffers 写回。
pub fn block_write_full_folio(
    page: &Arc<Page>,
    bdev: &Arc<dyn BlockDevice>,
) -> Result<(), SystemError> {
    let mut guard = page.write_irqsave();

    // 检查页面是否有 buffer heads
    let bh = match guard.buffers() {
        Some(bh) => bh,
        None => return Ok(()), // 没有 buffers，无需写回
    };

    // 标记页面为 writeback
    guard.add_flags(PageFlags::PG_WRITEBACK);

    // 遍历所有 buffer heads
    for buffer in BufferHeadIter::new(bh) {
        if buffer.is_dirty() && buffer.is_mapped() {
            // 获取块号
            let block_nr = buffer.blocknr();

            // 准备数据
            let data = unsafe {
                let slice = guard.as_slice();
                &slice[buffer.data_offset()..buffer.data_offset() + buffer.size()]
            };

            // 写入块设备
            bdev.write_at_sync(block_nr as usize, 1, data)?;

            // 清除脏标志
            buffer.clear_dirty();
        }
    }

    // 清除页面的脏和 writeback 标志
    guard.remove_flags(PageFlags::PG_DIRTY | PageFlags::PG_WRITEBACK);

    Ok(())
}

/// 从块设备读取整个页面
///
/// 这是一个辅助函数，提供简化的页面读取接口。
pub fn block_read_full_folio(
    page: &Arc<Page>,
    bdev: &Arc<dyn BlockDevice>,
    block_size: usize,
) -> Result<(), SystemError> {
    // 获取页面索引
    let page_index = {
        let guard = page.read_irqsave();
        match guard.page_type() {
            PageType::File(info) => info.index,
            _ => return Err(SystemError::EINVAL),
        }
    };

    // 确保页面有 buffer heads
    let head_bh = {
        let mut guard = page.write_irqsave();
        if !guard.has_buffers() {
            let bh = create_empty_buffers(page, block_size)?;
            guard.attach_buffers(bh.clone());
            bh
        } else {
            guard.buffers().unwrap()
        }
    };

    // 计算块参数
    let blocks_per_page = MMArch::PAGE_SIZE / block_size;
    let start_block = page_index * blocks_per_page;

    // 读取每个 buffer
    let mut block_idx = 0;
    for bh in BufferHeadIter::new(head_bh) {
        if block_idx >= blocks_per_page {
            break;
        }

        let block_nr = (start_block + block_idx) as u64;

        // 映射并读取
        bh.map_to_block(bdev, block_nr);

        let mut guard = page.write_irqsave();
        let data = unsafe {
            let slice = guard.as_slice_mut();
            &mut slice[bh.data_offset()..bh.data_offset() + bh.size()]
        };

        bdev.read_at_sync(block_nr as usize, 1, data)?;
        bh.set_uptodate();

        block_idx += 1;
    }

    // 标记页面为 uptodate
    page.write_irqsave().add_flags(PageFlags::PG_UPTODATE);

    Ok(())
}
