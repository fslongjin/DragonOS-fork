//! 新调度子系统 MVP 实现
//!
//! 本模块实现了一个简化的调度子系统，采用 RR（时间片轮转）+ IDLE 的调度策略。
//! 设计目标是最小化改动，快速验证新框架，与旧调度器通过 feature flag 共存。
//!
//! ## 架构设计
//!
//! ```text
//! ┌─────────────────────────────────────────────────────┐
//! │            ClassScheduler (全局调度器)               │
//! │  ┌───────────────────────────────────────────────┐  │
//! │  │  PerCpuClassRq (每 CPU 运行队列)               │  │
//! │  │  ├── RoundRobinClassRq  (时间片轮转)           │  │
//! │  │  └── IdleClassRq        (IDLE 调度)           │  │
//! │  └───────────────────────────────────────────────┘  │
//! └─────────────────────────────────────────────────────┘
//! ```
//!
//! ## 使用方式
//!
//! 通过 feature flag `sched_new` 启用新调度器：
//! ```bash
//! make FEATURES=sched_new
//! ```

mod class_rq;
mod entity;
mod idle;
mod rr;
mod scheduler;

pub use class_rq::PerCpuClassRq;
pub use entity::{SchedEntity, SchedState};
pub use idle::IdleClassRq;
pub use rr::RoundRobinClassRq;
pub use scheduler::{
    cpu_rq, do_schedule, force_switch_to_idle, sched_enqueue, sched_init, sched_sleep, sched_tick,
    sched_yield, schedule, scheduler, set_idle_task, wakeup, ClassScheduler, SchedMode,
};

use alloc::sync::Arc;
use core::time::Duration;

use crate::process::ProcessControlBlock;

/// 入队标志
bitflags::bitflags! {
    pub struct EnqueueFlags: u8 {
        /// 新创建的任务
        const SPAWN = 0x01;
        /// 被唤醒的任务
        const WAKE = 0x02;
        /// 任务迁移
        const MIGRATE = 0x04;
    }
}

/// 更新标志
bitflags::bitflags! {
    pub struct UpdateFlags: u8 {
        /// 时钟 tick
        const TICK = 0x01;
        /// 主动让出
        const YIELD = 0x02;
        /// 进入等待
        const WAIT = 0x04;
        /// 任务退出
        const EXIT = 0x08;
    }
}

/// 调度策略
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedPolicy {
    /// 时间片轮转（普通任务）
    RoundRobin,
    /// IDLE 任务
    Idle,
}

impl Default for SchedPolicy {
    fn default() -> Self {
        SchedPolicy::RoundRobin
    }
}

/// 调度类运行队列接口
pub trait SchedClassRq: Send {
    /// 将实体加入队列
    fn enqueue(&mut self, entity: Arc<SchedEntity>, flags: EnqueueFlags);

    /// 从队列移除实体
    fn dequeue(&mut self, entity: &Arc<SchedEntity>);

    /// 选择下一个实体
    fn pick_next(&mut self) -> Option<Arc<SchedEntity>>;

    /// 将选出的实体放回队列（如果需要继续运行）
    fn put_prev(&mut self, entity: Arc<SchedEntity>);

    /// 更新当前实体
    /// 返回是否需要在该调度类内切换
    fn update_current(&mut self, entity: &Arc<SchedEntity>, delta: Duration, flags: UpdateFlags)
        -> bool;

    /// 是否有就绪任务
    fn has_runnable(&self) -> bool;

    /// 队列长度
    fn len(&self) -> usize;

    /// 队列是否为空
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 本地运行队列接口
pub trait LocalRunQueue: Send {
    /// 获取当前正在运行的实体
    fn current(&self) -> Option<Arc<SchedEntity>>;

    /// 获取当前运行实体对应的 PCB
    fn current_pcb(&self) -> Option<Arc<ProcessControlBlock>>;

    /// 更新当前任务状态
    /// 返回是否需要切换任务
    fn update_current(&mut self, flags: UpdateFlags) -> bool;

    /// 选择下一个任务
    fn pick_next(&mut self) -> Option<Arc<SchedEntity>>;

    /// 将当前任务放回队列
    fn put_prev_task(&mut self);

    /// 将任务入队
    fn enqueue(&mut self, entity: Arc<SchedEntity>, flags: EnqueueFlags);

    /// 将任务出队
    fn dequeue(&mut self, entity: &Arc<SchedEntity>);

    /// 队列中的任务数量
    fn nr_running(&self) -> usize;
}
