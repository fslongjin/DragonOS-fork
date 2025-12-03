//! 每 CPU 调度类运行队列集合 (PerCpuClassRq)
//!
//! 聚合所有调度类的运行队列，按优先级选择任务：RR -> IDLE

use alloc::sync::Arc;
use core::time::Duration;

use crate::{process::ProcessControlBlock, smp::cpu::ProcessorId, time::timer::clock};

use super::{
    EnqueueFlags, IdleClassRq, LocalRunQueue, RoundRobinClassRq, SchedClassRq, SchedEntity,
    SchedPolicy, UpdateFlags,
};

/// 每 CPU 调度类运行队列集合
#[derive(Debug)]
pub struct PerCpuClassRq {
    /// CPU ID
    cpu: ProcessorId,
    /// RR 调度类（时间片轮转，MVP 核心）
    rr: RoundRobinClassRq,
    /// IDLE 调度类（最低优先级）
    idle: IdleClassRq,
    /// 当前运行的实体
    current: Option<Arc<SchedEntity>>,
    /// 当前任务开始执行的时间戳（jiffies）
    current_start: u64,
    /// 总运行任务数（不包括 IDLE）
    nr_running: usize,
}

impl PerCpuClassRq {
    /// 创建新的每 CPU 运行队列
    pub fn new(cpu: ProcessorId) -> Self {
        Self {
            cpu,
            rr: RoundRobinClassRq::new(),
            idle: IdleClassRq::new(),
            current: None,
            current_start: 0,
            nr_running: 0,
        }
    }

    /// 获取 CPU ID
    #[inline]
    pub fn cpu(&self) -> ProcessorId {
        self.cpu
    }

    /// 设置 IDLE 任务
    pub fn set_idle_task(&mut self, entity: Arc<SchedEntity>) {
        self.idle.set_idle(entity);
    }

    /// 获取 IDLE 任务
    pub fn idle_task(&self) -> Option<&Arc<SchedEntity>> {
        self.idle.idle()
    }

    /// 设置当前运行的任务（用于初始化）
    pub fn set_current(&mut self, entity: Arc<SchedEntity>) {
        entity.mark_running();
        entity.set_cpu(self.cpu.data());
        self.current = Some(entity);
        self.current_start = clock();
    }

    /// 根据调度策略选择对应的调度类队列
    fn get_policy(entity: &SchedEntity) -> SchedPolicy {
        if entity.is_idle() {
            SchedPolicy::Idle
        } else {
            SchedPolicy::RoundRobin
        }
    }

    /// 将任务入队到对应的调度类
    fn enqueue_entity_internal(&mut self, entity: Arc<SchedEntity>, flags: EnqueueFlags) {
        let policy = Self::get_policy(&entity);
        match policy {
            SchedPolicy::RoundRobin => {
                self.rr.enqueue(entity, flags);
                self.nr_running += 1;
            }
            SchedPolicy::Idle => {
                self.idle.enqueue(entity, flags);
                // IDLE 不计入 nr_running
            }
        }
    }

    /// 将任务从队列移除
    fn dequeue_entity_internal(&mut self, entity: &Arc<SchedEntity>) {
        let policy = Self::get_policy(entity);
        match policy {
            SchedPolicy::RoundRobin => {
                self.rr.dequeue(entity);
                self.nr_running = self.nr_running.saturating_sub(1);
            }
            SchedPolicy::Idle => {
                self.idle.dequeue(entity);
            }
        }
    }

    /// 选择下一个任务（按优先级：RR -> IDLE）
    fn pick_next_entity(&mut self) -> Option<Arc<SchedEntity>> {
        // 1. 优先从 RR 队列选择
        if let Some(entity) = self.rr.pick_next() {
            return Some(entity);
        }

        // 2. 没有普通任务，返回 IDLE
        self.idle.pick_next()
    }

    /// 将当前任务放回对应的调度类队列
    fn put_prev_entity(&mut self, entity: Arc<SchedEntity>) {
        let policy = Self::get_policy(&entity);
        
        match policy {
            SchedPolicy::RoundRobin => {
                // 如果任务还是可运行状态，放回队列
                if entity.state().is_runnable() {
                    self.rr.put_prev(entity);
                } else {
                    // 任务睡眠/退出，减少计数
                    self.nr_running = self.nr_running.saturating_sub(1);
                }
            }
            SchedPolicy::Idle => {
                self.idle.put_prev(entity);
            }
        }
    }

    /// 更新当前任务状态
    fn update_current_entity(&mut self, flags: UpdateFlags) -> bool {
        let Some(current) = self.current.as_ref() else {
            return false;
        };

        // 计算运行时间
        let now = clock();
        let delta_ticks = now.saturating_sub(self.current_start);
        // 转换为纳秒（假设 HZ=1000）
        let delta_ns = Duration::from_millis(delta_ticks);

        let policy = Self::get_policy(current);
        let need_resched = match policy {
            SchedPolicy::RoundRobin => self.rr.update_current(current, delta_ns, flags),
            SchedPolicy::Idle => self.idle.update_current(current, delta_ns, flags),
        };

        need_resched
    }

    /// 检查是否需要抢占当前任务
    pub fn check_preempt(&self, new_entity: &SchedEntity) -> bool {
        let Some(current) = self.current.as_ref() else {
            return true; // 没有当前任务，需要调度
        };

        let current_policy = Self::get_policy(current);
        let new_policy = Self::get_policy(new_entity);

        // RR 任务可以抢占 IDLE 任务
        if new_policy == SchedPolicy::RoundRobin && current_policy == SchedPolicy::Idle {
            return true;
        }

        false
    }
}

impl LocalRunQueue for PerCpuClassRq {
    fn current(&self) -> Option<Arc<SchedEntity>> {
        self.current.clone()
    }

    fn current_pcb(&self) -> Option<Arc<ProcessControlBlock>> {
        self.current.as_ref().and_then(|e| e.pcb())
    }

    fn update_current(&mut self, flags: UpdateFlags) -> bool {
        self.update_current_entity(flags)
    }

    fn pick_next(&mut self) -> Option<Arc<SchedEntity>> {
        // 1. 处理当前任务
        if let Some(prev) = self.current.take() {
            let state = prev.state();
            // 只有当前任务是 Running 状态且非退出状态时，才标记为 Runnable 并放回队列
            // 如果任务是睡眠状态（Interruptible/Uninterruptible），不要修改其状态
            if state.is_running() && !state.is_exited() {
                prev.mark_runnable();
            }
            self.put_prev_entity(prev);
        }

        // 2. 选择下一个任务
        let next = self.pick_next_entity()?;

        // 3. 设置新的当前任务
        next.mark_running();
        next.set_cpu(self.cpu.data());
        self.current = Some(next.clone());
        self.current_start = clock();

        Some(next)
    }

    fn put_prev_task(&mut self) {
        if let Some(prev) = self.current.take() {
            let state = prev.state();
            // 只有当前任务是 Running 状态且非退出状态时，才标记为 Runnable 并放回队列
            // 如果任务是睡眠状态（Interruptible/Uninterruptible），不要修改其状态
            if state.is_running() && !state.is_exited() {
                prev.mark_runnable();
            }
            self.put_prev_entity(prev);
        }
    }

    fn enqueue(&mut self, entity: Arc<SchedEntity>, flags: EnqueueFlags) {
        entity.set_on_rq(true);
        entity.set_cpu(self.cpu.data());
        self.enqueue_entity_internal(entity, flags);
    }

    fn dequeue(&mut self, entity: &Arc<SchedEntity>) {
        entity.set_on_rq(false);
        self.dequeue_entity_internal(entity);
    }

    fn nr_running(&self) -> usize {
        self.nr_running
    }
}
