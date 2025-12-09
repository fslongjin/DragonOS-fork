//! 调度实体 (SchedEntity) - MVP 简化版
//!
//! 设计原则：最小化字段，够用即可。
//! 调度实体从 ProcessControlBlock 中抽离，包含调度所需的核心信息。

use alloc::sync::{Arc, Weak};
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};

use crate::{process::ProcessControlBlock, smp::cpu::ProcessorId};
use super::SchedPolicy;

/// 调度状态
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedState {
    /// 正在运行
    Running = 0,
    /// 可运行（在队列中）
    Runnable = 1,
    /// 可中断睡眠
    Interruptible = 2,
    /// 不可中断睡眠
    Uninterruptible = 3,
    /// 已退出
    Exited = 4,
    /// 作业控制停止（对应 SIGSTOP/SIGTSTP 等）
    Stopped = 5,
}

impl SchedState {
    /// 从 u8 转换
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => SchedState::Running,
            1 => SchedState::Runnable,
            2 => SchedState::Interruptible,
            3 => SchedState::Uninterruptible,
            4 => SchedState::Exited,
            5 => SchedState::Stopped,
            _ => SchedState::Runnable, // 默认
        }
    }

    /// 是否可以被唤醒（普通信号唤醒，不包括 Stopped）
    pub fn is_wakeable(&self) -> bool {
        matches!(
            self,
            SchedState::Interruptible | SchedState::Uninterruptible
        )
    }

    pub fn is_interruptible(&self) -> bool {
        matches!(self, SchedState::Interruptible)
    }

    pub fn is_uninterruptible(&self) -> bool {
        matches!(self, SchedState::Uninterruptible)
    }
    /// 是否正在睡眠（包括 Stopped）
    pub fn is_sleeping(&self) -> bool {
        matches!(
            self,
            SchedState::Interruptible | SchedState::Uninterruptible
        )
    }

    /// 是否正在运行
    pub fn is_running(&self) -> bool {
        matches!(self, SchedState::Running)
    }

    /// 是否可以运行
    pub fn is_runnable(&self) -> bool {
        matches!(self, SchedState::Running | SchedState::Runnable)
    }

    /// 是否已退出
    pub fn is_exited(&self) -> bool {
        matches!(self, SchedState::Exited)
    }

    /// 是否处于 Stopped 状态
    pub fn is_stopped(&self) -> bool {
        matches!(self, SchedState::Stopped)
    }
}

impl Default for SchedState {
    fn default() -> Self {
        SchedState::Runnable
    }
}

/// 调度实体 - MVP 简化版
///
/// 设计原则：最小化字段，够用即可。
/// 所有字段使用原子操作，避免锁竞争。
#[derive(Debug)]
pub struct SchedEntity {
    /// 调度状态（原子操作）
    state: AtomicU8,
    /// 调度策略（RR / Idle）
    policy: AtomicU8,

    /// 当前/最近运行的 CPU（u32::MAX 表示未绑定）
    cpu: AtomicU32,

    /// 时间片（纳秒）
    slice: AtomicU64,

    /// 当前时间片已运行时间（纳秒）
    runtime: AtomicU64,

    /// 关联的 PCB（弱引用）
    pcb: Weak<ProcessControlBlock>,

    /// 是否在运行队列中
    on_rq: AtomicU8,
}

impl SchedEntity {
    /// 默认时间片：10ms (纳秒)
    pub const DEFAULT_SLICE_NS: u64 = 10_000_000;

    /// 创建新的调度实体
    ///
    /// 初始状态为 Uninterruptible（不可中断睡眠），这样 fork 后可以通过 wakeup 正确入队。
    /// wakeup() 函数只会对 is_wakeable() 状态（Interruptible/Uninterruptible）的任务入队。
    pub fn new(pcb: Weak<ProcessControlBlock>) -> Arc<Self> {
        Arc::new(Self {
            // 初始状态设为 Uninterruptible，这样 wakeup 可以正常工作
            state: AtomicU8::new(SchedState::Uninterruptible as u8),
            policy: AtomicU8::new(SchedPolicy::RoundRobin as u8),
            cpu: AtomicU32::new(ProcessorId::NONE.data()),
            slice: AtomicU64::new(Self::DEFAULT_SLICE_NS),
            runtime: AtomicU64::new(0),
            pcb,
            on_rq: AtomicU8::new(0),
        })
    }

    /// 创建 IDLE 任务的调度实体
    pub fn new_idle(pcb: Weak<ProcessControlBlock>) -> Arc<Self> {
        Arc::new(Self {
            state: AtomicU8::new(SchedState::Runnable as u8),
            policy: AtomicU8::new(SchedPolicy::Idle as u8),
            cpu: AtomicU32::new(ProcessorId::NONE.data()),
            slice: AtomicU64::new(u64::MAX), // IDLE 任务不需要时间片限制
            runtime: AtomicU64::new(0),
            pcb,
            on_rq: AtomicU8::new(1), // IDLE 始终在队列中
        })
    }

    /// 获取调度状态
    #[inline]
    pub fn state(&self) -> SchedState {
        SchedState::from_u8(self.state.load(Ordering::Acquire))
    }

    /// 设置调度状态
    #[inline]
    pub fn set_state(&self, state: SchedState) {
        self.state.store(state as u8, Ordering::Release);
    }

    /// 原子地比较并设置状态
    /// 返回是否设置成功
    #[inline]
    pub fn try_set_state(&self, expected: SchedState, new: SchedState) -> bool {
        self.state
            .compare_exchange(
                expected as u8,
                new as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    /// 获取当前 CPU
    #[inline]
    pub fn cpu(&self) -> ProcessorId {
        let cpu = self.cpu.load(Ordering::Acquire);
        ProcessorId::new(cpu)
    }

    /// 获取调度策略
    #[inline]
    pub fn policy(&self) -> SchedPolicy {
        match self.policy.load(Ordering::Acquire) {
            x if x == (SchedPolicy::Idle as u8) => SchedPolicy::Idle,
            _ => SchedPolicy::RoundRobin,
        }
    }

    /// 设置调度策略
    #[inline]
    pub fn set_policy(&self, policy: SchedPolicy) {
        self.policy.store(policy as u8, Ordering::Release);
    }

    /// 设置当前 CPU
    #[inline]
    pub fn set_cpu(&self, cpu: ProcessorId) {
        self.cpu.store(cpu.data(), Ordering::Release);
    }

    /// 清除 CPU 绑定
    #[inline]
    pub fn clear_cpu(&self) {
        self.cpu.store(ProcessorId::NONE.data(), Ordering::Release);
    }

    /// 原子地尝试设置 CPU（如果当前未绑定）
    /// 返回是否设置成功
    #[inline]
    pub fn try_set_cpu_if_none(&self, cpu: ProcessorId) -> bool {
        self.cpu
            .compare_exchange(ProcessorId::NONE.data(), cpu.data(), Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// 获取时间片（纳秒）
    #[inline]
    pub fn slice(&self) -> u64 {
        self.slice.load(Ordering::Relaxed)
    }

    /// 设置时间片（纳秒）
    #[inline]
    pub fn set_slice(&self, slice: u64) {
        self.slice.store(slice, Ordering::Relaxed);
    }

    /// 获取已运行时间（纳秒）
    #[inline]
    pub fn runtime(&self) -> u64 {
        self.runtime.load(Ordering::Relaxed)
    }

    /// 重置时间片（用于重新入队时）
    #[inline]
    pub fn reset_slice(&self) {
        self.runtime.store(0, Ordering::Relaxed);
    }

    /// 增加运行时间，返回是否超时（时间片用完）
    #[inline]
    pub fn charge_runtime(&self, delta_ns: u64) -> bool {
        let old_runtime = self.runtime.fetch_add(delta_ns, Ordering::Relaxed);
        let new_runtime = old_runtime.saturating_add(delta_ns);
        new_runtime >= self.slice.load(Ordering::Relaxed)
    }

    /// 获取关联的 PCB
    #[inline]
    pub fn pcb(&self) -> Option<Arc<ProcessControlBlock>> {
        self.pcb.upgrade()
    }

    /// 获取 PCB 的弱引用
    #[inline]
    pub fn pcb_weak(&self) -> &Weak<ProcessControlBlock> {
        &self.pcb
    }

    /// 是否在运行队列中
    #[inline]
    pub fn is_on_rq(&self) -> bool {
        self.on_rq.load(Ordering::Acquire) != 0
    }

    /// 设置是否在运行队列中
    #[inline]
    pub fn set_on_rq(&self, on_rq: bool) {
        self.on_rq.store(on_rq as u8, Ordering::Release);
    }

    /// 标记为正在运行
    #[inline]
    pub fn mark_running(&self) {
        self.set_state(SchedState::Running);
    }

    /// 标记为可运行
    #[inline]
    pub fn mark_runnable(&self) {
        self.set_state(SchedState::Runnable);
    }

    /// 标记为可中断睡眠
    #[inline]
    pub fn mark_interruptible(&self) {
        self.set_state(SchedState::Interruptible);
    }

    /// 标记为不可中断睡眠
    #[inline]
    pub fn mark_uninterruptible(&self) {
        self.set_state(SchedState::Uninterruptible);
    }

    /// 标记为已退出
    #[inline]
    pub fn mark_exited(&self) {
        self.set_state(SchedState::Exited);
    }

    /// 标记为已停止（用于作业控制）
    #[inline]
    pub fn mark_stopped(&self) {
        self.set_state(SchedState::Stopped);
    }

    /// 判断是否是 IDLE 任务（根据策略）
    #[inline]
    pub fn is_idle(&self) -> bool {
        matches!(self.policy(), SchedPolicy::Idle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sched_state() {
        assert!(SchedState::Interruptible.is_wakeable());
        assert!(SchedState::Uninterruptible.is_wakeable());
        assert!(!SchedState::Running.is_wakeable());
        assert!(!SchedState::Runnable.is_wakeable());
        assert!(!SchedState::Exited.is_wakeable());

        assert!(SchedState::Running.is_runnable());
        assert!(SchedState::Runnable.is_runnable());
        assert!(!SchedState::Interruptible.is_runnable());
    }
}
