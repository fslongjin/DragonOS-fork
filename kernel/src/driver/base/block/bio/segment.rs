//! BioSegment - Bio内存段
//!
//! BioSegment表示一个Bio请求中的一段内存，用于scatter-gather IO。

use alloc::{sync::Arc, vec::Vec};

use crate::arch::MMArch;
use crate::mm::page::Page;
use crate::mm::MemoryManagementArch;

/// Bio 内存段
///
/// 表示一个IO操作涉及的内存区域
#[derive(Clone)]
pub struct BioSegment {
    /// 关联的物理页
    page: Arc<Page>,
    /// 页内偏移
    offset: usize,
    /// 数据长度
    len: usize,
}

impl BioSegment {
    /// 创建一个新的BioSegment
    ///
    /// # 参数
    /// - `page`: 物理页
    /// - `offset`: 页内偏移
    /// - `len`: 数据长度
    pub fn new(page: Arc<Page>, offset: usize, len: usize) -> Self {
        BioSegment { page, offset, len }
    }

    /// 从页面创建整页的BioSegment
    pub fn from_page(page: Arc<Page>) -> Self {
        BioSegment {
            page,
            offset: 0,
            len: MMArch::PAGE_SIZE,
        }
    }

    /// 获取关联的页面
    #[inline]
    pub fn page(&self) -> &Arc<Page> {
        &self.page
    }

    /// 获取页内偏移
    #[inline]
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// 获取数据长度
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// 检查是否为空
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 读取数据到缓冲区
    pub fn read_to_buf(&self, buf: &mut [u8]) -> usize {
        let read_len = core::cmp::min(buf.len(), self.len);
        let page_guard = self.page.read_irqsave();
        unsafe {
            let src = page_guard.as_slice();
            buf[..read_len].copy_from_slice(&src[self.offset..self.offset + read_len]);
        }
        read_len
    }

    /// 从缓冲区写入数据
    pub fn write_from_buf(&self, buf: &[u8]) -> usize {
        let write_len = core::cmp::min(buf.len(), self.len);
        let mut page_guard = self.page.write_irqsave();
        unsafe {
            let dst = page_guard.as_slice_mut();
            dst[self.offset..self.offset + write_len].copy_from_slice(&buf[..write_len]);
        }
        write_len
    }
}

/// BioSegment 构建器
///
/// 用于方便地构建多个BioSegment
pub struct BioSegmentBuilder {
    segments: Vec<BioSegment>,
}

impl BioSegmentBuilder {
    /// 创建新的构建器
    pub fn new() -> Self {
        BioSegmentBuilder {
            segments: Vec::new(),
        }
    }

    /// 添加一个整页
    pub fn add_page(mut self, page: Arc<Page>) -> Self {
        self.segments.push(BioSegment::from_page(page));
        self
    }

    /// 添加一个部分页
    pub fn add_partial(mut self, page: Arc<Page>, offset: usize, len: usize) -> Self {
        self.segments.push(BioSegment::new(page, offset, len));
        self
    }

    /// 构建segment列表
    pub fn build(self) -> Vec<BioSegment> {
        self.segments
    }
}

impl Default for BioSegmentBuilder {
    fn default() -> Self {
        Self::new()
    }
}
