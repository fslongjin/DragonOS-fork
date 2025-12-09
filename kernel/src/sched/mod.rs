pub mod clock;
pub mod completion;
pub mod cputime;
pub mod prio;
pub mod syscall;

use core::{
    intrinsics::{likely, unlikely},
    sync::atomic::{compiler_fence, fence, AtomicUsize, Ordering},
};

use alloc::{
    boxed::Box,
    collections::LinkedList,
    sync::{Arc, Weak},
    vec::Vec,
};
use system_error::SystemError;

use crate::{
    arch::{interrupt::ipi::send_ipi, CurrentIrqArch},
    exception::{
        ipi::{IpiKind, IpiTarget},
        InterruptArch,
    },
    libs::{
        lazy_init::Lazy,
        spinlock::{SpinLock, SpinLockGuard},
    },
    mm::percpu::{PerCpu, PerCpuVar},
    process::{ProcessControlBlock, ProcessFlags, ProcessManager, ProcessState, SchedInfo},
    smp::{core::smp_get_processor_id, cpu::ProcessorId},
    time::{clocksource::HZ, timer::clock},
};

use self::{
    clock::{ClockUpdataFlag, SchedClock},
    cputime::{irq_time_read, CpuTimeFunc, IrqTime},
    prio::PrioUtil,
};

static mut CPU_IRQ_TIME: Option<Vec<&'static mut IrqTime>> = None;

/// 用于记录系统中所有 CPU 的可执行进程数量的总和。
static CALCULATE_LOAD_TASKS: AtomicUsize = AtomicUsize::new(0);

const LOAD_FREQ: usize = HZ as usize * 5 + 1;

pub const SCHED_FIXEDPOINT_SHIFT: u64 = 10;
#[allow(dead_code)]
pub const SCHED_FIXEDPOINT_SCALE: u64 = 1 << SCHED_FIXEDPOINT_SHIFT;
#[allow(dead_code)]
pub const SCHED_CAPACITY_SHIFT: u64 = SCHED_FIXEDPOINT_SHIFT;
#[allow(dead_code)]
pub const SCHED_CAPACITY_SCALE: u64 = 1 << SCHED_CAPACITY_SHIFT;

#[inline]
pub fn cpu_irq_time(cpu: ProcessorId) -> &'static mut IrqTime {
    unsafe { CPU_IRQ_TIME.as_mut().unwrap()[cpu.data() as usize] }
}

lazy_static! {
    pub static ref SCHED_FEATURES: SchedFeature = SchedFeature::GENTLE_FAIR_SLEEPERS
        | SchedFeature::START_DEBIT
        | SchedFeature::LAST_BUDDY
        | SchedFeature::CACHE_HOT_BUDDY
        | SchedFeature::WAKEUP_PREEMPTION
        | SchedFeature::NONTASK_CAPACITY
        | SchedFeature::TTWU_QUEUE
        | SchedFeature::SIS_UTIL
        | SchedFeature::RT_PUSH_IPI
        | SchedFeature::ALT_PERIOD
        | SchedFeature::BASE_SLICE
        | SchedFeature::UTIL_EST
        | SchedFeature::UTIL_EST_FASTUP;
}

/// 调度策略
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OldSchedPolicy {
    /// 实时进程
    RT,
    /// 先进先出调度
    FIFO,
    /// 完全公平调度
    CFS,
    /// IDLE
    IDLE,
}

#[derive(Debug, Default)]
pub struct LoadWeight {
    /// 负载权重
    pub weight: u64,
    /// weight的倒数，方便计算
    pub inv_weight: u32,
}

impl LoadWeight {
    /// 用于限制权重在一个合适的区域内
    pub const SCHED_FIXEDPOINT_SHIFT: u32 = 10;

    pub const WMULT_SHIFT: u32 = 32;
    pub const WMULT_CONST: u32 = !0;

    pub const NICE_0_LOAD_SHIFT: u32 = Self::SCHED_FIXEDPOINT_SHIFT + Self::SCHED_FIXEDPOINT_SHIFT;

    pub fn update_load_add(&mut self, inc: u64) {
        self.weight += inc;
        self.inv_weight = 0;
    }

    pub fn update_load_sub(&mut self, dec: u64) {
        self.weight -= dec;
        self.inv_weight = 0;
    }

    pub fn update_load_set(&mut self, weight: u64) {
        self.weight = weight;
        self.inv_weight = 0;
    }

    /// ## 更新负载权重的倒数
    pub fn update_inv_weight(&mut self) {
        // 已经更新
        if likely(self.inv_weight != 0) {
            return;
        }

        let w = Self::scale_load_down(self.weight);

        if unlikely(w >= Self::WMULT_CONST as u64) {
            // 高位有数据
            self.inv_weight = 1;
        } else if unlikely(w == 0) {
            // 倒数去最大
            self.inv_weight = Self::WMULT_CONST;
        } else {
            // 计算倒数
            self.inv_weight = Self::WMULT_CONST / w as u32;
        }
    }

    /// ## 计算任务的执行时间差
    ///
    /// 计算公式：(delta_exec * (weight * self.inv_weight)) >> WMULT_SHIFT
    pub fn calculate_delta(&mut self, delta_exec: u64, weight: u64) -> u64 {
        // 降低精度
        let mut fact = Self::scale_load_down(weight);

        // 记录fact高32位
        let mut fact_hi = (fact >> 32) as u32;
        // 用于恢复
        let mut shift = Self::WMULT_SHIFT;

        self.update_inv_weight();

        if unlikely(fact_hi != 0) {
            // 这里表示高32位还有数据
            // 需要计算最高位，然后继续调整fact
            let fs = 32 - fact_hi.leading_zeros();
            shift -= fs;

            // 确保高32位全为0
            fact >>= fs;
        }

        // 这里确定了fact已经在32位内
        fact *= self.inv_weight as u64;

        fact_hi = (fact >> 32) as u32;

        if fact_hi != 0 {
            // 这里表示高32位还有数据
            // 需要计算最高位，然后继续调整fact
            let fs = 32 - fact_hi.leading_zeros();
            shift -= fs;

            // 确保高32位全为0
            fact >>= fs;
        }

        return ((delta_exec as u128 * fact as u128) >> shift) as u64;
    }

    /// ## 将负载权重缩小到到一个小的范围中计算，相当于减小精度计算
    pub const fn scale_load_down(mut weight: u64) -> u64 {
        if weight != 0 {
            weight >>= Self::SCHED_FIXEDPOINT_SHIFT;

            if weight < 2 {
                weight = 2;
            }
        }
        weight
    }

    #[allow(dead_code)]
    pub const fn scale_load(weight: u64) -> u64 {
        weight << Self::SCHED_FIXEDPOINT_SHIFT
    }
}

pub trait SchedArch {
    /// 开启当前核心的调度
    fn enable_sched_local();
    /// 关闭当前核心的调度
    #[allow(dead_code)]
    fn disable_sched_local();

    /// 在第一次开启调度之前，进行初始化工作。
    ///
    /// 注意区别于sched_init，这个函数只是做初始化时钟的工作等等。
    fn initial_setup_sched_local() {}
}

bitflags! {
    pub struct SchedFeature:u32 {
        /// 给予睡眠任务仅有 50% 的服务赤字。这意味着睡眠任务在被唤醒后会获得一定的服务，但不能过多地占用资源。
        const GENTLE_FAIR_SLEEPERS = 1 << 0;
        /// 将新任务排在前面，以避免已经运行的任务被饿死
        const START_DEBIT = 1 << 1;
        /// 在调度时优先选择上次唤醒的任务，因为它可能会访问之前唤醒的任务所使用的数据，从而提高缓存局部性。
        const NEXT_BUDDY = 1 << 2;
        /// 在调度时优先选择上次运行的任务，因为它可能会访问与之前运行的任务相同的数据，从而提高缓存局部性。
        const LAST_BUDDY = 1 << 3;
        /// 认为任务的伙伴（buddy）在缓存中是热点，减少缓存伙伴被迁移的可能性，从而提高缓存局部性。
        const CACHE_HOT_BUDDY = 1 << 4;
        /// 允许唤醒时抢占当前任务。
        const WAKEUP_PREEMPTION = 1 << 5;
        /// 基于任务未运行时间来减少 CPU 的容量。
        const NONTASK_CAPACITY = 1 << 6;
        /// 将远程唤醒排队到目标 CPU，并使用调度器 IPI 处理它们，以减少运行队列锁的争用。
        const TTWU_QUEUE = 1 << 7;
        /// 在唤醒时尝试限制对最后级联缓存（LLC）域的无谓扫描。
        const SIS_UTIL = 1 << 8;
        /// 在 RT（Real-Time）任务迁移时，通过发送 IPI 来减少 CPU 之间的锁竞争。
        const RT_PUSH_IPI = 1 << 9;
        /// 启用估计的 CPU 利用率功能，用于调度决策。
        const UTIL_EST = 1 << 10;
        const UTIL_EST_FASTUP = 1 << 11;
        /// 启用备选调度周期
        const ALT_PERIOD = 1 << 12;
        /// 启用基本时间片
        const BASE_SLICE = 1 << 13;
    }

    pub struct EnqueueFlag: u8 {
        const ENQUEUE_WAKEUP	= 0x01;
        const ENQUEUE_RESTORE	= 0x02;
        const ENQUEUE_MOVE	= 0x04;
        const ENQUEUE_NOCLOCK	= 0x08;

        const ENQUEUE_MIGRATED	= 0x40;

        const ENQUEUE_INITIAL	= 0x80;
    }

    pub struct DequeueFlag: u8 {
        const DEQUEUE_SLEEP		= 0x01;
        const DEQUEUE_SAVE		= 0x02; /* Matches ENQUEUE_RESTORE */
        const DEQUEUE_MOVE		= 0x04; /* Matches ENQUEUE_MOVE */
        const DEQUEUE_NOCLOCK		= 0x08; /* Matches ENQUEUE_NOCLOCK */
    }

    pub struct WakeupFlags: u8 {
        /* Wake flags. The first three directly map to some SD flag value */
        const WF_EXEC         = 0x02; /* Wakeup after exec; maps to SD_BALANCE_EXEC */
        const WF_FORK         = 0x04; /* Wakeup after fork; maps to SD_BALANCE_FORK */
        const WF_TTWU         = 0x08; /* Wakeup;            maps to SD_BALANCE_WAKE */

        const WF_SYNC         = 0x10; /* Waker goes to sleep after wakeup */
        const WF_MIGRATED     = 0x20; /* Internal use, task got migrated */
        const WF_CURRENT_CPU  = 0x40; /* Prefer to move the wakee to the current CPU. */
    }

    pub struct SchedMode: u8 {
        /*
        * Constants for the sched_mode argument of __schedule().
        *
        * The mode argument allows RT enabled kernels to differentiate a
        * preemption from blocking on an 'sleeping' spin/rwlock. Note that
        * SM_MASK_PREEMPT for !RT has all bits set, which allows the compiler to
        * optimize the AND operation out and just check for zero.
        */
        /// 在调度过程中不会再次进入队列，即需要手动唤醒
        const SM_NONE			= 0x0;
        /// 重新加入队列，即当前进程被抢占，需要时钟调度
        const SM_PREEMPT		= 0x1;
        /// rt相关
        const SM_RTLOCK_WAIT		= 0x2;
        /// 默认与SM_PREEMPT相同
        const SM_MASK_PREEMPT	= Self::SM_PREEMPT.bits;
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum OnRq {
    Queued,
    Migrating,
    None,
}

impl ProcessManager {
    pub fn update_process_times(user_tick: bool) {
        let pcb = Self::current_pcb();
        CpuTimeFunc::irqtime_account_process_tick(&pcb, user_tick, 1);

        scheduler_tick();
    }
}

/// ## 时钟tick时调用此函数
pub fn scheduler_tick() {
    crate::sched_new::sched_tick();
}

/// ## 执行调度
/// 若preempt_count不为0则报错
#[inline]
pub fn schedule(sched_mod: SchedMode) {
    let _guard = unsafe { CurrentIrqArch::save_and_disable_irq() };
    assert_eq!(ProcessManager::current_pcb().preempt_count(), 0);

    let mode = if sched_mod.contains(SchedMode::SM_MASK_PREEMPT) {
        crate::sched_new::SchedMode::Preempt
    } else {
        crate::sched_new::SchedMode::None
    };
    crate::sched_new::schedule(mode);
    return;
}

/// ## 执行调度
/// 此函数与schedule的区别为，该函数不会检查preempt_count
/// 适用于时钟中断等场景
pub fn __schedule(sched_mod: SchedMode) {
    let _irq_guard = unsafe { CurrentIrqArch::save_and_disable_irq() };
    let mode = if sched_mod.contains(SchedMode::SM_MASK_PREEMPT) {
        crate::sched_new::SchedMode::Preempt
    } else {
        crate::sched_new::SchedMode::None
    };
    crate::sched_new::do_schedule(mode);
    return;
}

pub fn sched_fork(pcb: &Arc<ProcessControlBlock>) -> Result<(), SystemError> {
    let mut prio_guard = pcb.sched_info().prio_data.write_irqsave();
    let current = ProcessManager::current_pcb();

    prio_guard.prio = current.sched_info().prio_data.read_irqsave().normal_prio;

    if PrioUtil::dl_prio(prio_guard.prio) {
        return Err(SystemError::EAGAIN_OR_EWOULDBLOCK);
    } else if PrioUtil::rt_prio(prio_guard.prio) {
        let policy = &pcb.sched_info().sched_policy;
        *policy.write_irqsave() = OldSchedPolicy::RT;
    } else {
        let policy = &pcb.sched_info().sched_policy;
        *policy.write_irqsave() = OldSchedPolicy::CFS;
    }

    Ok(())
}

pub fn sched_cgroup_fork(pcb: &Arc<ProcessControlBlock>) {
    return;
}

#[inline(never)]
pub fn sched_init() {
    // 初始化percpu变量
    unsafe {
        CPU_IRQ_TIME = Some(Vec::with_capacity(PerCpu::MAX_CPU_NUM as usize));
        CPU_IRQ_TIME
            .as_mut()
            .unwrap()
            .resize_with(PerCpu::MAX_CPU_NUM as usize, || Box::leak(Box::default()));
    };
}

#[inline]
pub fn send_resched_ipi(cpu: ProcessorId) {
    send_ipi(IpiKind::KickCpu, IpiTarget::Specified(cpu));
}
