//! 调度器核心实现 (ClassScheduler)
//!
//! 全局调度器，管理所有 CPU 的运行队列，提供调度入口函数。

use alloc::{sync::Arc, vec::Vec};
use core::sync::atomic::{fence, AtomicU32, Ordering};

use crate::{
    arch::CurrentIrqArch,
    exception::InterruptArch,
    libs::{lazy_init::Lazy, spinlock::SpinLock},
    mm::percpu::{PerCpu, PerCpuVar},
    process::{ProcessControlBlock, ProcessFlags, ProcessManager},
    smp::{
        core::smp_get_processor_id,
        cpu::{try_smp_cpu_manager, ProcessorId},
    },
};

use super::{EnqueueFlags, LocalRunQueue, PerCpuClassRq, SchedEntity, UpdateFlags};

/// 全局调度器实例
static SCHEDULER: Lazy<ClassScheduler> = Lazy::new();

/// 每 CPU 运行队列
static CPU_RQ: Lazy<PerCpuVar<SpinLock<PerCpuClassRq>>> = PerCpuVar::define_lazy();

/// 调度模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedMode {
    /// 不重新入队（睡眠/退出）
    None,
    /// 被抢占，重新入队
    Preempt,
}

/// 类调度器 - 多调度类聚合
pub struct ClassScheduler {
    /// CPU 数量
    nr_cpus: usize,
    /// 上次选择的 CPU（简单负载均衡用）
    last_chosen_cpu: AtomicU32,
}

impl ClassScheduler {
    /// 创建新的调度器
    fn new(nr_cpus: usize) -> Self {
        Self {
            nr_cpus,
            last_chosen_cpu: AtomicU32::new(0),
        }
    }

    /// 选择最优 CPU
    ///
    /// 当前阶段的需求：只有 idle 可以在非 0 号 CPU 上运行，其他任务强制投递到 CPU0。
    pub fn select_cpu(&self, entity: &SchedEntity, _flags: EnqueueFlags) -> ProcessorId {
        // 非 idle 任务全部绑定 CPU0，避免被投递到其它 CPU
        if !entity.is_idle() {
            return ProcessorId::new(0);
        }

        // idle 任务：若已绑定 CPU 则复用，否则使用当前 CPU
        let idle_cpu = entity.cpu();
        if idle_cpu != ProcessorId::NONE {
            return idle_cpu;
        }
        smp_get_processor_id()
    }

    /// 选择负载最低的 online CPU（需要 SMP 管理器）
    fn select_least_loaded_cpu_with_manager(
        &self,
        smp_manager: &'static crate::smp::cpu::SmpCpuManager,
    ) -> ProcessorId {
        let mut min_load = usize::MAX;
        // 默认选择 CPU 0（BSP 始终 online）
        let mut selected = ProcessorId::new(0);

        // 从 last_chosen_cpu 开始轮询，避免总是选择同一个 CPU
        let start = self.last_chosen_cpu.load(Ordering::Relaxed) as usize;

        for i in 0..self.nr_cpus {
            let cpu_idx = (start + i) % self.nr_cpus;
            let cpu = ProcessorId::new(cpu_idx as u32);

            // 跳过 offline 的 CPU
            if !smp_manager.is_cpu_online(cpu) {
                continue;
            }

            let rq = cpu_rq(cpu);
            let guard = rq.lock();
            let load = guard.nr_running();

            if load < min_load {
                min_load = load;
                selected = cpu;

                // 如果找到空闲 CPU，立即返回
                if load == 0 {
                    break;
                }
            }
        }

        self.last_chosen_cpu
            .store(selected.data(), Ordering::Relaxed);
        selected
    }

    /// 选择负载最低的 online CPU
    fn select_least_loaded_cpu(&self) -> ProcessorId {
        // 如果 SMP 管理器还未初始化，只使用 CPU 0
        match try_smp_cpu_manager() {
            Some(m) => self.select_least_loaded_cpu_with_manager(m),
            None => ProcessorId::new(0),
        }
    }
}

/// 获取全局调度器
#[inline]
pub fn scheduler() -> &'static ClassScheduler {
    SCHEDULER.ensure();
    unsafe { SCHEDULER.get_unchecked() }
}

/// 获取指定 CPU 的运行队列
#[inline]
pub fn cpu_rq(cpu: ProcessorId) -> &'static SpinLock<PerCpuClassRq> {
    CPU_RQ.ensure();
    unsafe { CPU_RQ.get().force_get(cpu) }
}

/// 获取当前 CPU 的运行队列
#[inline]
pub fn this_rq() -> &'static SpinLock<PerCpuClassRq> {
    cpu_rq(smp_get_processor_id())
}

/// 初始化调度子系统
pub fn sched_init() {
    log::info!("sched_new: Initializing new scheduler (RR + IDLE)");

    let nr_cpus = PerCpu::MAX_CPU_NUM as usize;

    // 初始化每 CPU 运行队列
    let mut rqs = Vec::with_capacity(nr_cpus);
    for cpu_idx in 0..nr_cpus {
        let cpu = ProcessorId::new(cpu_idx as u32);
        let rq = SpinLock::new(PerCpuClassRq::new(cpu));
        rqs.push(rq);
    }

    CPU_RQ.init(PerCpuVar::new(rqs).unwrap());

    // 初始化全局调度器
    SCHEDULER.init(ClassScheduler::new(nr_cpus));

    log::info!("sched_new: Scheduler initialized with {} CPUs", nr_cpus);
}

/// 将任务加入调度
///
/// 返回应该被抢占的 CPU（如果有）
pub fn sched_enqueue(entity: &Arc<SchedEntity>, flags: EnqueueFlags) -> Option<ProcessorId> {
    let _irq_guard = unsafe { CurrentIrqArch::save_and_disable_irq() };

    // 已退出的任务不应再入队
    if entity.state().is_exited() {
        let pid = entity.pcb().map(|p| p.raw_pid().data());
        log::warn!("sched_enqueue: skip exited entity pid={:?}", pid);
        return None;
    }

    // 1. 选择目标 CPU
    let target_cpu = scheduler().select_cpu(entity, flags);

    // 2. 获取目标 CPU 的运行队列并入队
    let rq = cpu_rq(target_cpu);
    let mut guard = rq.lock();

    // 3. 检查是否需要抢占
    let need_preempt = guard.check_preempt(entity);

    let pid = entity.pcb().map(|p| p.raw_pid().data()).unwrap_or(9999);
    let nr_before = guard.nr_running();

    // 4. 入队
    guard.enqueue(entity.clone(), flags);

    let nr_after = guard.nr_running();
    // log::debug!(
    //     "sched_enqueue: pid={} target_cpu={} need_preempt={} nr_running: {} -> {}",
    //     pid,
    //     target_cpu.data(),
    //     need_preempt,
    //     nr_before,
    //     nr_after
    // );

    drop(guard);

    // 5. 如果需要抢占且不是当前 CPU，返回 CPU ID
    if need_preempt {
        let current_cpu = smp_get_processor_id();
        if target_cpu != current_cpu {
            return Some(target_cpu);
        } else {
            // 当前 CPU 需要抢占，设置标志
            ProcessManager::current_pcb()
                .flags()
                .insert(ProcessFlags::NEED_SCHEDULE);
        }
    }

    None
}

/// 唤醒任务
pub fn wakeup(entity: &Arc<SchedEntity>) -> bool {
    // 检查状态
    let state = entity.state();
    if state.is_exited() {
        let pid = entity.pcb().map(|p| p.raw_pid().data());
        log::warn!("sched_new::wakeup: pid={:?} already exited, skip", pid);
        return false;
    }
    if !state.is_wakeable() {
        let pid = entity.pcb().map(|p| p.raw_pid().data()).unwrap_or(9999);
        // log::debug!(
        //     "sched_new::wakeup: pid={} state={:?} NOT wakeable, skip",
        //     pid,
        //     state
        // );
        return false;
    }

    let pid = entity.pcb().map(|p| p.raw_pid().data()).unwrap_or(9999);
    // log::debug!(
    //     "sched_new::wakeup: pid={} state={:?} -> Runnable",
    //     pid,
    //     state
    // );

    // 设置为可运行
    entity.mark_runnable();

    // 入队
    let preempt_cpu = sched_enqueue(entity, EnqueueFlags::WAKE);

    // 如果需要抢占远程 CPU，发送 IPI
    if let Some(cpu) = preempt_cpu {
        send_resched_ipi(cpu);
    }

    true
}

/// 时钟 tick 处理
pub fn sched_tick() {
    let _irq_guard = unsafe { CurrentIrqArch::save_and_disable_irq() };

    let rq = this_rq();
    let mut guard = rq.lock();

    // 更新当前任务
    let need_resched = guard.update_current(UpdateFlags::TICK);

    let nr_running = guard.nr_running();
    drop(guard);

    if need_resched {
        let pid = ProcessManager::current_pcb().raw_pid().data();
        log::debug!(
            "sched_tick: pid={} need_resched=true nr_running={}",
            pid,
            nr_running
        );
        // 设置需要调度标志
        ProcessManager::current_pcb()
            .flags()
            .insert(ProcessFlags::NEED_SCHEDULE);
    }
}

/// 执行调度
pub fn schedule(mode: SchedMode) {
    let _irq_guard = unsafe { CurrentIrqArch::save_and_disable_irq() };

    // 检查 preempt_count
    let pcb = ProcessManager::current_pcb();
    if pcb.preempt_count() != 0 {
        let state = pcb.sched_info().sched_entity().state();
        if state.is_exited() {
            log::warn!(
                "schedule(): preempt_count {} while task(pid={}) is exiting, force resched",
                pcb.preempt_count(),
                pcb.raw_pid().data()
            );
        } else {
            log::warn!("schedule() called with preempt_count != 0");
            return;
        }
    }

    do_schedule(mode);
}

/// 内部调度实现（不检查 preempt_count）
pub fn do_schedule(mode: SchedMode) {
    let cpu = smp_get_processor_id();
    let rq = cpu_rq(cpu);

    let _irq_guard = unsafe { CurrentIrqArch::save_and_disable_irq() };
    let mut guard = rq.lock();

    // 获取当前任务
    let prev_pcb = ProcessManager::current_pcb();
    let prev_entity = guard.current();

    // 根据模式决定是否更新状态
    let update_flags = match mode {
        SchedMode::None => UpdateFlags::WAIT,
        SchedMode::Preempt => UpdateFlags::TICK,
    };

    // 更新当前任务状态
    guard.update_current(update_flags);

    // 如果当前任务已退出，直接将其从运行队列移除并清空 current，然后立即重新选任务
    if let Some(ref prev_entity) = prev_entity {
        if prev_entity.state().is_exited() {
            let pid = prev_entity.pcb().map(|p| p.raw_pid().data());
            log::warn!(
                "do_schedule: current entity already exited pid={:?}, removing",
                pid
            );
            guard.dequeue(prev_entity);
            guard.clear_current();
            // 彻底清理退出任务的运行队列状态，避免被错误重选
            prev_entity.set_on_rq(false);
            prev_entity.clear_cpu();

            // 直接重新选下一个任务，避免后续逻辑再处理已退出任务
            let mut next_entity = loop {
                let picked = guard.pick_next_direct();
                let Some(ent) = picked else {
                    log::error!("sched_new: No task to run after removing exited current");
                    return;
                };
                if ent.state().is_exited() || ent.pcb().is_none() {
                    let pid = ent.pcb().map(|p| p.raw_pid().data());
                    log::warn!("sched_new: drop picked exited/none pcb entity pid={:?}", pid);
                    guard.dequeue(&ent);
                    continue;
                }
                break ent;
            };

            let Some(next_pcb) = next_entity.pcb() else {
                log::error!("sched_new: Next entity has no PCB after removing exited current");
                return;
            };

            // 清除需要调度标志
            prev_pcb.flags().remove(ProcessFlags::NEED_SCHEDULE);
            fence(Ordering::SeqCst);

            // 设置新的当前任务
            next_entity.mark_running();
            next_entity.set_cpu(cpu);
            guard.set_current(next_entity.clone());

            drop(guard);

            if !Arc::ptr_eq(&prev_pcb, &next_pcb) {
                unsafe { ProcessManager::switch_process(prev_pcb, next_pcb) };
            }
            return;
        }
    }

    // 选择下一个任务
    let mut next_entity = loop {
        let picked = guard.pick_next();
        let Some(ent) = picked else {
            // 不应该发生：至少有 IDLE 任务
            log::error!("sched_new: No task to run!");
            return;
        };

        // 若选中退出/无 PCB 的实体，直接丢弃后继续选
        if ent.state().is_exited() || ent.pcb().is_none() {
            let pid = ent.pcb().map(|p| p.raw_pid().data());
            log::warn!("sched_new: drop picked exited/none pcb entity pid={:?}", pid);
            guard.dequeue(&ent);
            continue;
        }

        // 若当前任务已退出且再次被选中，强制改选 idle/其他
        if let Some(prev_ent) = prev_entity.as_ref() {
            if Arc::ptr_eq(prev_ent, &ent) && prev_ent.state().is_exited() {
                guard.dequeue(prev_ent);
                guard.clear_current();
                continue;
            }
        }

        break ent;
    };

    // 防御：如果仍然拿到了刚刚退出的任务（理论不该发生），强制切到 idle
    if let Some(prev_ent) = prev_entity.as_ref() {
        if prev_ent.state().is_exited() && Arc::ptr_eq(prev_ent, &next_entity) {
            if let Some(idle_ent) = guard.idle_entity() {
                log::error!(
                    "sched_new: picked exited entity again (pid={:?}), forcing idle",
                    prev_ent.pcb().map(|p| p.raw_pid().data())
                );
                next_entity = idle_ent;
            } else {
                log::error!("sched_new: no idle entity to recover from exited pick, halt");
                loop {
                    core::hint::spin_loop();
                }
            }
        }
    }

    // 若仍然与 prev 相同且 prev 处于退出态，尝试直接切换到 idle
    if let Some(prev_ent) = prev_entity.as_ref() {
        if Arc::ptr_eq(prev_ent, &next_entity) && prev_ent.state().is_exited() {
            if let Some(idle_ent) = guard.idle_entity() {
                next_entity = idle_ent;
            }
        }
    }

    let Some(next_pcb) = next_entity.pcb() else {
        log::error!("sched_new: Next entity has no PCB!");
        return;
    };

    // 清除需要调度标志
    prev_pcb.flags().remove(ProcessFlags::NEED_SCHEDULE);
    fence(Ordering::SeqCst);

    // 如果是不同的任务，进行上下文切换
    if !Arc::ptr_eq(&prev_pcb, &next_pcb) {
        let next_pid = next_pcb.raw_pid().data();
        // log::debug!(
        //     "sched_new: switch cpu={} prev_pid={} prev_state={:?} -> next_pid={} nr_running={}",
        //     cpu.data(),
        //     prev_pid,
        //     prev_sched_state,
        //     next_pid,
        //     guard.nr_running()
        // );
        drop(guard);

        // 执行上下文切换
        unsafe {
            ProcessManager::switch_process(prev_pcb, next_pcb);
        }
    }
}

/// 发送重调度 IPI
fn send_resched_ipi(cpu: ProcessorId) {
    use crate::arch::interrupt::ipi::send_ipi;
    use crate::exception::ipi::{IpiKind, IpiTarget};

    send_ipi(IpiKind::KickCpu, IpiTarget::Specified(cpu));
}

/// 设置 IDLE 任务
pub fn set_idle_task(cpu: ProcessorId, entity: Arc<SchedEntity>) {
    let rq = cpu_rq(cpu);
    let mut guard = rq.lock();
    guard.set_idle_task(entity.clone());
    // 同时设置为当前任务（初始时 idle 是当前运行的任务）
    guard.set_current(entity);
}

/// 强制将当前 CPU 切换到 IDLE，不返回
///
/// 用于退出路径的兜底保护，确保当前已退出实体不会再次运行。
/// 调用者需保证当前任务已标记退出。
pub fn force_switch_to_idle(prev_pcb: Arc<ProcessControlBlock>) -> ! {
    let cpu = smp_get_processor_id();
    let rq = cpu_rq(cpu);

    let _irq_guard = unsafe { CurrentIrqArch::save_and_disable_irq() };
    let mut guard = rq.lock();

    let idle_ent = guard.idle_entity().unwrap_or_else(|| {
        log::error!("force_switch_to_idle: no idle entity on cpu {}", cpu.data());
        loop {
            core::hint::spin_loop();
        }
    });
    let idle_pcb = idle_ent
        .pcb()
        .unwrap_or_else(|| panic!("force_switch_to_idle: idle entity without pcb"));

    // 将 idle 设置为当前，确保运行队列视图一致
    guard.set_current(idle_ent.clone());
    drop(guard);

    // 直接切换上下文；不应返回
    unsafe { ProcessManager::switch_process(prev_pcb, idle_pcb) };
    unreachable!("force_switch_to_idle: returned unexpectedly");
}

/// 当前任务主动让出 CPU
pub fn sched_yield() {
    let _irq_guard = unsafe { CurrentIrqArch::save_and_disable_irq() };

    let rq = this_rq();
    let mut guard = rq.lock();

    // 标记为让出
    guard.update_current(UpdateFlags::YIELD);

    drop(guard);

    // 设置需要调度标志并调度
    ProcessManager::current_pcb()
        .flags()
        .insert(ProcessFlags::NEED_SCHEDULE);

    do_schedule(SchedMode::Preempt);
}

/// 当前任务进入睡眠
pub fn sched_sleep(interruptible: bool) {
    let _irq_guard = unsafe { CurrentIrqArch::save_and_disable_irq() };

    let rq = this_rq();
    let mut guard = rq.lock();

    // 获取当前实体并设置状态
    if let Some(entity) = guard.current() {
        if interruptible {
            entity.mark_interruptible();
        } else {
            entity.mark_uninterruptible();
        }
    }

    // 更新并出队
    guard.update_current(UpdateFlags::WAIT);

    drop(guard);

    // 调度
    do_schedule(SchedMode::None);
}
