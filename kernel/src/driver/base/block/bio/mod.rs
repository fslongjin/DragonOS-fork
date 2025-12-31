//! Block I/O (Bio) 抽象层
//!
//! 参考 Asterinas 的设计，实现统一的块IO请求抽象。
//! Bio层负责：
//! - 封装IO请求
//! - 支持请求合并
//! - 异步IO等待
//! - 内存池管理
//!
//! # 设计思想
//! - 每个Bio代表一个块IO请求
//! - BioSegment代表IO操作涉及的内存段
//! - BioWaiter支持批量等待多个Bio完成
//! - BioSegmentPool提供预分配内存池

mod pool;
mod request_queue;
mod segment;

pub use pool::*;
pub use request_queue::*;
pub use segment::*;

use alloc::{sync::Arc, vec::Vec};
use core::{
    ops::Range,
    sync::atomic::{AtomicU32, Ordering},
};
use system_error::SystemError;

use crate::libs::wait_queue::WaitQueue;

/// 扇区ID类型 (LBA)
pub type Sid = usize;

/// 块设备一个扇区的大小（字节）
pub const SECTOR_SIZE: usize = 512;

/// Bio 类型
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum BioType {
    /// 读操作
    Read = 0,
    /// 写操作
    Write = 1,
    /// 刷新操作
    Flush = 2,
    /// 丢弃操作（TRIM）
    Discard = 3,
}

/// Bio 状态
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum BioStatus {
    /// 初始化状态
    Init = 0,
    /// 已提交
    Submitted = 1,
    /// 已完成
    Completed = 2,
    /// 不支持的操作
    NotSupported = 3,
    /// 空间不足
    NoSpace = 4,
    /// IO错误
    IoError = 5,
}

impl From<u32> for BioStatus {
    fn from(val: u32) -> Self {
        match val {
            0 => BioStatus::Init,
            1 => BioStatus::Submitted,
            2 => BioStatus::Completed,
            3 => BioStatus::NotSupported,
            4 => BioStatus::NoSpace,
            _ => BioStatus::IoError,
        }
    }
}

/// Block I/O 请求
///
/// 封装一个块IO操作的所有信息
pub struct Bio {
    inner: Arc<BioInner>,
}

struct BioInner {
    /// IO类型
    bio_type: BioType,
    /// 扇区范围 (起始LBA, 结束LBA)
    sector_range: Range<Sid>,
    /// 内存段列表
    segments: Vec<BioSegment>,
    /// 当前状态
    status: AtomicU32,
    /// 完成等待队列
    wait_queue: WaitQueue,
}

impl Bio {
    /// 创建一个新的Bio请求
    ///
    /// # 参数
    /// - `bio_type`: IO操作类型
    /// - `sector_start`: 起始扇区号
    /// - `segments`: 内存段列表
    pub fn new(bio_type: BioType, sector_start: Sid, segments: Vec<BioSegment>) -> Self {
        let total_sectors: usize = segments.iter().map(|seg| seg.len() / SECTOR_SIZE).sum();
        let sector_end = sector_start + total_sectors;

        Bio {
            inner: Arc::new(BioInner {
                bio_type,
                sector_range: sector_start..sector_end,
                segments,
                status: AtomicU32::new(BioStatus::Init as u32),
                wait_queue: WaitQueue::default(),
            }),
        }
    }

    /// 获取IO类型
    #[inline]
    pub fn bio_type(&self) -> BioType {
        self.inner.bio_type
    }

    /// 获取扇区范围
    #[inline]
    pub fn sector_range(&self) -> &Range<Sid> {
        &self.inner.sector_range
    }

    /// 获取起始扇区
    #[inline]
    pub fn sector_start(&self) -> Sid {
        self.inner.sector_range.start
    }

    /// 获取结束扇区
    #[inline]
    pub fn sector_end(&self) -> Sid {
        self.inner.sector_range.end
    }

    /// 获取扇区数量
    #[inline]
    pub fn sector_count(&self) -> usize {
        self.inner.sector_range.end - self.inner.sector_range.start
    }

    /// 获取内存段
    #[inline]
    pub fn segments(&self) -> &[BioSegment] {
        &self.inner.segments
    }

    /// 获取当前状态
    #[inline]
    pub fn status(&self) -> BioStatus {
        BioStatus::from(self.inner.status.load(Ordering::Acquire))
    }

    /// 设置状态
    fn set_status(&self, status: BioStatus) {
        self.inner.status.store(status as u32, Ordering::Release);
    }

    /// 标记Bio已提交
    pub fn submit(&self) {
        self.set_status(BioStatus::Submitted);
    }

    /// 标记Bio已完成
    pub fn complete(&self, status: BioStatus) {
        self.set_status(status);
        self.inner.wait_queue.wakeup_all(None);
    }

    /// 等待Bio完成
    pub fn wait(&self) -> Result<(), SystemError> {
        // 使用wait_event等待完成
        self.inner.wait_queue.wait_event_interruptible(
            || {
                let status = self.status();
                matches!(
                    status,
                    BioStatus::Completed
                        | BioStatus::IoError
                        | BioStatus::NotSupported
                        | BioStatus::NoSpace
                )
            },
            None::<fn()>,
        )?;

        match self.status() {
            BioStatus::Completed => Ok(()),
            BioStatus::IoError => Err(SystemError::EIO),
            BioStatus::NotSupported => Err(SystemError::ENOSYS),
            BioStatus::NoSpace => Err(SystemError::ENOSPC),
            _ => Ok(()),
        }
    }

    /// 获取内部Arc引用
    pub(crate) fn inner_arc(&self) -> Arc<BioInner> {
        self.inner.clone()
    }
}

impl Clone for Bio {
    fn clone(&self) -> Self {
        Bio {
            inner: self.inner.clone(),
        }
    }
}

/// 已提交的Bio包装器
pub struct SubmittedBio {
    bio: Bio,
}

impl SubmittedBio {
    /// 从Bio创建SubmittedBio
    pub fn new(bio: Bio) -> Self {
        bio.submit();
        SubmittedBio { bio }
    }

    /// 获取IO类型
    #[inline]
    pub fn bio_type(&self) -> BioType {
        self.bio.bio_type()
    }

    /// 获取扇区范围
    #[inline]
    pub fn sector_range(&self) -> &Range<Sid> {
        self.bio.sector_range()
    }

    /// 获取内存段
    #[inline]
    pub fn segments(&self) -> &[BioSegment] {
        self.bio.segments()
    }

    /// 完成Bio
    pub fn complete(self, status: BioStatus) {
        self.bio.complete(status);
    }

    /// 获取内部Bio
    pub fn bio(&self) -> &Bio {
        &self.bio
    }
}

/// Bio等待器
///
/// 支持批量等待多个Bio完成
pub struct BioWaiter {
    bios: Vec<Bio>,
}

impl BioWaiter {
    /// 创建一个新的BioWaiter
    pub fn new() -> Self {
        BioWaiter { bios: Vec::new() }
    }

    /// 添加一个Bio到等待列表
    pub fn add(&mut self, bio: Bio) {
        self.bios.push(bio);
    }

    /// 等待所有Bio完成
    pub fn wait(&self) -> Result<(), SystemError> {
        for bio in &self.bios {
            bio.wait()?;
        }
        Ok(())
    }

    /// 获取Bio数量
    pub fn len(&self) -> usize {
        self.bios.len()
    }

    /// 检查是否为空
    pub fn is_empty(&self) -> bool {
        self.bios.is_empty()
    }

    /// 合并另一个BioWaiter
    pub fn concat(&mut self, other: BioWaiter) {
        self.bios.extend(other.bios);
    }
}

impl Default for BioWaiter {
    fn default() -> Self {
        Self::new()
    }
}
