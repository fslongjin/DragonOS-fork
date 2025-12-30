//! Page Cache Operations
//!
//! 定义 PageCache 的操作接口，类似于 Linux 的 address_space_operations。
//! 不同类型的后端（文件系统、块设备）可以实现自己的操作。

use core::fmt::Debug;

use alloc::sync::Arc;
use system_error::SystemError;

use super::page::Page;

/// 回写控制参数
#[derive(Debug, Clone)]
pub struct WritebackControl {
    /// 同步模式
    pub sync_mode: WbSyncMode,
    /// 最大写入页数
    pub nr_to_write: usize,
    /// 起始字节位置
    pub range_start: u64,
    /// 结束字节位置
    pub range_end: u64,
    /// 是否循环回写
    pub range_cyclic: bool,
    /// 是否使用 TOWRITE 标签
    pub tagged_writepages: bool,
}

impl Default for WritebackControl {
    fn default() -> Self {
        Self {
            sync_mode: WbSyncMode::None,
            nr_to_write: usize::MAX,
            range_start: 0,
            range_end: u64::MAX,
            range_cyclic: false,
            tagged_writepages: false,
        }
    }
}

impl WritebackControl {
    /// 创建用于 fsync 的回写控制
    pub fn for_sync(start: u64, end: u64) -> Self {
        Self {
            sync_mode: WbSyncMode::All,
            nr_to_write: usize::MAX,
            range_start: start,
            range_end: end,
            range_cyclic: false,
            tagged_writepages: true,
        }
    }

    /// 创建用于后台回写的控制
    pub fn for_background(nr_pages: usize) -> Self {
        Self {
            sync_mode: WbSyncMode::None,
            nr_to_write: nr_pages,
            range_start: 0,
            range_end: u64::MAX,
            range_cyclic: true,
            tagged_writepages: false,
        }
    }
}

/// 回写同步模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WbSyncMode {
    /// 不等待 I/O 完成
    None,
    /// 等待所有 I/O 完成（用于 fsync）
    All,
}

/// PageCache 操作接口
///
/// 类似于 Linux 的 address_space_operations，定义了 PageCache 与后端存储
/// 交互的标准接口。
pub trait PageCacheOperations: Send + Sync + Debug {
    /// 读取页面数据
    ///
    /// 当页面不在缓存中或需要从后端读取时调用。
    /// 实现应该填充页面数据并设置 PG_UPTODATE 标志。
    fn read_folio(&self, page: &Arc<Page>) -> Result<(), SystemError>;

    /// 写回多个脏页
    ///
    /// 实现应该遍历脏页并将它们写回后端存储。
    fn writepages(&self, wbc: &mut WritebackControl) -> Result<(), SystemError>;

    /// 标记页面为脏
    ///
    /// 返回 true 如果页面是新变脏的，false 如果已经是脏的。
    fn dirty_folio(&self, page: &Arc<Page>) -> bool;

    /// 释放页面
    ///
    /// 当页面要被回收时调用。实现可以释放与页面关联的私有数据。
    /// 返回 true 如果页面可以被释放，false 如果不能。
    fn release_folio(&self, page: &Arc<Page>) -> bool {
        // 默认实现：总是允许释放
        let _ = page;
        true
    }

    /// 使页面失效
    ///
    /// 当文件被截断或页面需要失效时调用。
    fn invalidate_folio(&self, page: &Arc<Page>, offset: usize, length: usize) {
        // 默认实现：什么都不做
        let _ = (page, offset, length);
    }

    /// 检查页面是否部分有效
    ///
    /// 用于支持部分页面读取优化。
    fn is_partially_uptodate(&self, page: &Arc<Page>, from: usize, count: usize) -> bool {
        // 默认实现：不支持部分有效
        let _ = (page, from, count);
        false
    }

    /// 检查页面的脏/回写状态
    ///
    /// 用于内存回收器判断页面状态。
    fn is_dirty_writeback(&self, page: &Arc<Page>) -> (bool, bool) {
        use super::page::PageFlags;

        let guard = page.read_irqsave();
        let flags = guard.flags();
        let dirty = flags.contains(PageFlags::PG_DIRTY);
        let writeback = flags.contains(PageFlags::PG_WRITEBACK);
        (dirty, writeback)
    }
}

/// 默认的 PageCache 操作（用于内存文件系统如 tmpfs）
#[derive(Debug)]
#[allow(dead_code)]
pub struct DefaultPageCacheOps;

impl PageCacheOperations for DefaultPageCacheOps {
    fn read_folio(&self, page: &Arc<Page>) -> Result<(), SystemError> {
        // 内存文件系统不需要从后端读取，页面数据直接在内存中
        use super::page::PageFlags;
        page.write_irqsave().add_flags(PageFlags::PG_UPTODATE);
        Ok(())
    }

    fn writepages(&self, _wbc: &mut WritebackControl) -> Result<(), SystemError> {
        // 内存文件系统不需要写回
        Ok(())
    }

    fn dirty_folio(&self, page: &Arc<Page>) -> bool {
        use super::page::PageFlags;

        let mut guard = page.write_irqsave();
        let was_dirty = guard.flags().contains(PageFlags::PG_DIRTY);
        if !was_dirty {
            guard.add_flags(PageFlags::PG_DIRTY);
        }
        !was_dirty
    }
}

/// 文件系统 PageCache 操作的基础实现
///
/// 提供文件系统通用的操作，具体文件系统可以继承或覆盖。
#[derive(Debug)]
#[allow(dead_code)]
pub struct FilesystemPageCacheOps {
    /// 块大小
    pub block_size: usize,
}

#[allow(dead_code)]
impl FilesystemPageCacheOps {
    pub fn new(block_size: usize) -> Self {
        Self { block_size }
    }
}

impl PageCacheOperations for FilesystemPageCacheOps {
    fn read_folio(&self, page: &Arc<Page>) -> Result<(), SystemError> {
        // 这个实现应该由具体的文件系统覆盖
        // 这里只是标记页面为有效
        use super::page::PageFlags;
        page.write_irqsave().add_flags(PageFlags::PG_UPTODATE);
        Ok(())
    }

    fn writepages(&self, _wbc: &mut WritebackControl) -> Result<(), SystemError> {
        // 这个实现应该由具体的文件系统覆盖
        Ok(())
    }

    fn dirty_folio(&self, page: &Arc<Page>) -> bool {
        use super::page::PageFlags;
        use super::buffer_head::BufferHeadIter;

        let mut guard = page.write_irqsave();

        // 如果页面有 buffer heads，标记所有 buffer 为脏
        if let Some(bh) = guard.buffers() {
            for buffer in BufferHeadIter::new(bh.clone()) {
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
        use super::buffer_head::BufferHeadIter;

        let guard = page.read_irqsave();

        // 检查是否有忙碌的 buffer heads
        if let Some(bh) = guard.buffers() {
            for buffer in BufferHeadIter::new(bh.clone()) {
                if buffer.is_busy() {
                    return false;
                }
            }
        }

        true
    }

    fn is_partially_uptodate(&self, page: &Arc<Page>, from: usize, count: usize) -> bool {
        use super::buffer_head::BufferHeadIter;

        let guard = page.read_irqsave();

        if let Some(bh) = guard.buffers() {
            let to = from + count;

            for buffer in BufferHeadIter::new(bh.clone()) {
                let bh_start = buffer.data_offset();
                let bh_end = bh_start + buffer.size();

                // 检查这个 buffer 是否与请求范围重叠
                if bh_end <= from {
                    continue;
                }
                if bh_start >= to {
                    break;
                }

                // 如果有重叠但 buffer 不是 uptodate，则返回 false
                if !buffer.is_uptodate() {
                    return false;
                }
            }
            return true;
        }

        false
    }
}
