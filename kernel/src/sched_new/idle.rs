//! IDLE 调度类
//!
//! 每 CPU 一个 IDLE 任务，优先级最低。
//! 当没有其他任务可运行时，运行 IDLE 任务。

use alloc::sync::Arc;
use core::time::Duration;

use super::{EnqueueFlags, SchedClassRq, SchedEntity, UpdateFlags};

/// IDLE 调度类运行队列
///
/// 每个 CPU 有且仅有一个 IDLE 任务。
#[derive(Debug, Default)]
pub struct IdleClassRq {
    /// IDLE 任务的调度实体
    idle_entity: Option<Arc<SchedEntity>>,
}

impl IdleClassRq {
    /// 创建新的 IDLE 调度队列
    pub fn new() -> Self {
        Self { idle_entity: None }
    }

    /// 设置 IDLE 任务
    pub fn set_idle(&mut self, entity: Arc<SchedEntity>) {
        self.idle_entity = Some(entity);
    }

    /// 获取 IDLE 任务
    pub fn idle(&self) -> Option<&Arc<SchedEntity>> {
        self.idle_entity.as_ref()
    }
}

impl SchedClassRq for IdleClassRq {
    fn enqueue(&mut self, entity: Arc<SchedEntity>, _flags: EnqueueFlags) {
        // IDLE 任务只有一个，直接设置
        self.idle_entity = Some(entity);
    }

    fn dequeue(&mut self, entity: &Arc<SchedEntity>) {
        // IDLE 任务不应该被移出队列
        if let Some(ref idle) = self.idle_entity {
            if Arc::ptr_eq(idle, entity) {
                // 通常不应该发生，但为了安全起见
                log::warn!("Attempt to dequeue IDLE task, ignored");
            }
        }
    }

    fn pick_next(&mut self) -> Option<Arc<SchedEntity>> {
        // 返回 IDLE 任务但不移除（IDLE 任务永远存在）
        self.idle_entity.clone()
    }

    fn put_prev(&mut self, _entity: Arc<SchedEntity>) {
        // IDLE 任务不需要放回队列，它始终存在
    }

    fn update_current(
        &mut self,
        _entity: &Arc<SchedEntity>,
        _delta: Duration,
        _flags: UpdateFlags,
    ) -> bool {
        // IDLE 任务永不主动让出，只能被抢占
        // 返回 false 表示不需要切换
        false
    }

    fn has_runnable(&self) -> bool {
        self.idle_entity.is_some()
    }

    fn len(&self) -> usize {
        if self.idle_entity.is_some() {
            1
        } else {
            0
        }
    }
}
