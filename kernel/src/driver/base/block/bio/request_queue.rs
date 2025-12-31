//! Bio请求队列
//!
//! 实现IO请求的排队和合并功能

use alloc::{collections::VecDeque, vec::Vec};

use super::{BioSegment, BioStatus, BioType, Sid, SubmittedBio};
use crate::libs::spinlock::SpinLock;
use crate::libs::wait_queue::WaitQueue;

/// 合并后的Bio请求
///
/// 将多个连续的Bio合并成一个请求，提高IO效率
pub struct BioRequest {
    /// IO类型
    bio_type: BioType,
    /// 扇区范围
    sector_start: Sid,
    sector_end: Sid,
    /// 合并的Bio列表
    bios: VecDeque<SubmittedBio>,
    /// 总段数
    num_segments: usize,
}

impl BioRequest {
    /// 从单个SubmittedBio创建请求
    pub fn new(bio: SubmittedBio) -> Self {
        let sector_start = bio.sector_range().start;
        let sector_end = bio.sector_range().end;
        let num_segments = bio.segments().len();
        let bio_type = bio.bio_type();

        let mut bios = VecDeque::new();
        bios.push_back(bio);

        BioRequest {
            bio_type,
            sector_start,
            sector_end,
            bios,
            num_segments,
        }
    }

    /// 获取IO类型
    #[inline]
    pub fn bio_type(&self) -> BioType {
        self.bio_type
    }

    /// 获取起始扇区
    #[inline]
    pub fn sector_start(&self) -> Sid {
        self.sector_start
    }

    /// 获取结束扇区
    #[inline]
    pub fn sector_end(&self) -> Sid {
        self.sector_end
    }

    /// 获取扇区数量
    #[inline]
    pub fn sector_count(&self) -> usize {
        self.sector_end - self.sector_start
    }

    /// 获取段数量
    #[inline]
    pub fn num_segments(&self) -> usize {
        self.num_segments
    }

    /// 检查是否可以与另一个Bio合并
    ///
    /// 合并条件：
    /// 1. 类型相同
    /// 2. 扇区范围连续
    pub fn can_merge(&self, bio: &SubmittedBio) -> bool {
        // 类型必须相同
        if bio.bio_type() != self.bio_type {
            return false;
        }

        let bio_range = bio.sector_range();

        // 检查是否可以向后合并
        if bio_range.start == self.sector_end {
            return true;
        }

        // 检查是否可以向前合并
        if bio_range.end == self.sector_start {
            return true;
        }

        false
    }

    /// 合并一个Bio到请求中
    ///
    /// 调用前应先使用 `can_merge` 检查
    pub fn merge_bio(&mut self, bio: SubmittedBio) {
        let bio_range = bio.sector_range().clone();
        self.num_segments += bio.segments().len();

        if bio_range.start == self.sector_end {
            // 向后合并
            self.sector_end = bio_range.end;
            self.bios.push_back(bio);
        } else if bio_range.end == self.sector_start {
            // 向前合并
            self.sector_start = bio_range.start;
            self.bios.push_front(bio);
        }
    }

    /// 收集所有段
    pub fn collect_segments(&self) -> Vec<BioSegment> {
        let mut segments = Vec::with_capacity(self.num_segments);
        for bio in &self.bios {
            segments.extend_from_slice(bio.segments());
        }
        segments
    }

    /// 完成请求中的所有Bio
    pub fn complete_all(self, status: BioStatus) {
        for bio in self.bios {
            bio.complete(status);
        }
    }

    /// 获取Bio数量
    pub fn bio_count(&self) -> usize {
        self.bios.len()
    }
}

/// Bio请求队列
///
/// 管理待处理的Bio请求，支持请求合并
pub struct BioRequestQueue {
    /// 请求队列
    queue: SpinLock<VecDeque<BioRequest>>,
    /// 每个Bio最大段数
    max_segments_per_bio: usize,
    /// 等待队列
    wait_queue: WaitQueue,
}

impl BioRequestQueue {
    /// 默认每个Bio最大段数
    pub const DEFAULT_MAX_SEGMENTS: usize = 128;

    /// 创建新的请求队列
    pub fn new() -> Self {
        BioRequestQueue {
            queue: SpinLock::new(VecDeque::new()),
            max_segments_per_bio: Self::DEFAULT_MAX_SEGMENTS,
            wait_queue: WaitQueue::default(),
        }
    }

    /// 使用指定的最大段数创建请求队列
    pub fn with_max_segments(max_segments: usize) -> Self {
        BioRequestQueue {
            queue: SpinLock::new(VecDeque::new()),
            max_segments_per_bio: max_segments,
            wait_queue: WaitQueue::default(),
        }
    }

    /// 入队一个Bio
    ///
    /// 会尝试与队列中的请求合并
    pub fn enqueue(&self, bio: SubmittedBio) {
        let mut queue = self.queue.lock();

        // 尝试与队首请求合并
        if let Some(front) = queue.front_mut() {
            if front.can_merge(&bio)
                && front.num_segments() + bio.segments().len() <= self.max_segments_per_bio
            {
                front.merge_bio(bio);
                return;
            }
        }

        // 无法合并，创建新请求
        queue.push_front(BioRequest::new(bio));

        drop(queue);

        // 唤醒等待的处理线程
        self.wait_queue.wake_one();
    }

    /// 出队一个请求
    pub fn dequeue(&self) -> Option<BioRequest> {
        self.queue.lock().pop_back()
    }

    /// 等待并出队一个请求
    pub fn wait_dequeue(&self) -> BioRequest {
        loop {
            if let Some(req) = self.dequeue() {
                return req;
            }
            // 等待新请求到来
            let _ = self
                .wait_queue
                .wait_event_interruptible(|| !self.is_empty(), None::<fn()>);
        }
    }

    /// 获取队列长度
    pub fn len(&self) -> usize {
        self.queue.lock().len()
    }

    /// 检查队列是否为空
    pub fn is_empty(&self) -> bool {
        self.queue.lock().is_empty()
    }
}

impl Default for BioRequestQueue {
    fn default() -> Self {
        Self::new()
    }
}
