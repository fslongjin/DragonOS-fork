# DragonOS 调度子系统重构方案

> 本文档基于对 Asterinas 调度子系统架构的深入分析，结合 DragonOS 现有的进程/线程管理和同步机制，提出一套完整的调度子系统重构方案。

---

## 1. 现状分析

### 1.1 DragonOS 现有调度子系统问题

通过对 DragonOS 现有代码的分析，发现以下核心问题：

#### 1.1.1 架构复杂度问题

1. **过度复杂的 Linux 风格实现**
   - 当前实现仿照 Linux CFS 调度器，包含 PELT 负载追踪、调度组（TaskGroup）等复杂机制
   - `FairSchedEntity` 包含大量字段：`load`, `deadline`, `min_deadline`, `vruntime`, `vlag`, `slice`, `avg` 等
   - `CfsRunQueue` 结构体包含约 30+ 字段，难以维护和调试

2. **调度与进程管理耦合过深**
   - `ProcessControlBlock` 中直接包含 `sched_info: ProcessSchedulerInfo`
   - `ProcessSchedulerInfo` 中嵌入了 `FairSchedEntity`、`on_rq`、`prio_data` 等调度专属字段
   - 调度策略切换、入队/出队等操作分散在多处

3. **锁机制复杂，易死锁**
   - `CpuRunQueue` 使用 `SpinLock` + `self_lock()` 的特殊模式
   - `force_mut()` 等 unsafe 方法大量使用，破坏 Rust 的安全保证
   - 中断上下文与任务上下文的锁竞争问题

#### 1.1.2 功能缺陷

1. **多核负载均衡未完善**
   - `select_cpu` 逻辑简单，仅基于 `last_cpu` 和队列长度
   - 没有周期性的负载均衡机制
   - 任务迁移路径不完整

2. **抢占机制不健壮**
   - 依赖 `ProcessFlags::NEED_SCHEDULE` 标志
   - 抢占点分散，不够统一

3. **调试困难**
   - 调度路径复杂，难以追踪问题
   - 大量 unsafe 代码块

### 1.2 DragonOS 现有架构核心组件

```
kernel/src/
├── process/
│   ├── mod.rs              # ProcessControlBlock, ProcessManager, ProcessSchedulerInfo
│   ├── idle.rs             # Idle 进程初始化
│   ├── fork.rs             # 进程创建
│   └── ...
├── sched/
│   ├── mod.rs              # CpuRunQueue, Scheduler trait, __schedule()
│   ├── fair.rs             # FairSchedEntity, CfsRunQueue, CompletelyFairScheduler
│   ├── idle.rs             # IdleScheduler
│   ├── pelt.rs             # 负载追踪
│   ├── prio.rs             # 优先级
│   ├── clock.rs            # 调度时钟
│   └── ...
└── libs/
    ├── wait_queue.rs       # WaitQueue
    ├── mutex.rs            # Mutex
    ├── spinlock.rs         # SpinLock
    └── ...
```

### 1.3 Asterinas 调度架构优点

基于架构分析文档，Asterinas 的设计有以下值得借鉴的优点：

1. **清晰的分层架构**
   - OSTD 层：提供与策略无关的通用接口（Task, Scheduler, LocalRunQueue）
   - Kernel 层：实现具体调度策略（ClassScheduler, 各调度类）

2. **调度器注入机制**
   - 通过 `inject_scheduler` 实现调度器的可插拔
   - 便于测试和扩展

3. **统一的调度事件模型**
   - `UpdateFlags`：Tick / Wait / Yield / Exit
   - `EnqueueFlags`：Spawn / Wake
   - 所有调度决策通过统一的事件驱动

4. **原子化的 CPU 绑定**
   - `AtomicCpuId` 确保任务不会被多个 CPU 同时调度
   - `set_if_is_none` 的 CAS 模式

5. **简洁的调度类框架**
   - `SchedClassRq` trait 定义调度类接口
   - 按优先级顺序选择：STOP → REALTIME → FAIR → IDLE

---

## 2. 重构目标

### 2.1 核心目标

1. **简化架构**：移除不必要的复杂度，保持核心功能
2. **模块化设计**：调度子系统与进程管理解耦
3. **安全优先**：最大化利用 Rust 的所有权和生命周期机制
4. **支持多核负载均衡**：从设计上保证可扩展性
5. **最小化对外部模块的影响**：保持接口兼容性

### 2.2 非目标（MVP 阶段）

1. 不追求 100% Linux 兼容
2. 不实现完整的 cgroup/调度组
3. MVP 阶段不实现 CFS，仅实现 RR + IDLE
4. 负载均衡留作后续扩展

---

## 3. 新架构设计

### 3.1 分层架构

```
┌────────────────────────────────────────────────────────────┐
│                    System Call Layer                        │
│        (sched_setscheduler, sched_yield, etc.)             │
└────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌────────────────────────────────────────────────────────────┐
│                  Scheduler Interface Layer                  │
│   ┌─────────────────────────────────────────────────────┐  │
│   │  GlobalScheduler (trait object: &amp;dyn Scheduler)     │  │
│   │  - enqueue(task, flags) -> Option<CpuId>            │  │
│   │  - local_rq(cpu) -> &amp;dyn LocalRunQueue              │  │
│   └─────────────────────────────────────────────────────┘  │
└────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌────────────────────────────────────────────────────────────┐
│                  Scheduler Implementation                   │
│   ┌─────────────────────────────────────────────────────┐  │
│   │            ClassScheduler (default impl)            │  │
│   │  ┌───────────────────────────────────────────────┐  │  │
│   │  │  PerCpuRunQueue                               │  │  │
│   │  │  ├── StopClassRq      (最高优先级)            │  │  │
│   │  │  ├── FairClassRq      (CFS 简化版)            │  │  │
│   │  │  └── IdleClassRq      (最低优先级)            │  │  │
│   │  └───────────────────────────────────────────────┘  │  │
│   └─────────────────────────────────────────────────────┘  │
└────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌────────────────────────────────────────────────────────────┐
│                     Task Abstraction                        │
│   ┌─────────────────────────────────────────────────────┐  │
│   │  SchedEntity (调度实体，从 PCB 中抽离)              │  │
│   │  - state: SchedState (Running/Runnable/Blocked/...)│  │
│   │  - cpu: AtomicCpuId                                 │  │
│   │  - policy: SchedPolicy                              │  │
│   │  - fair_data: FairSchedData (vruntime, weight, ..) │  │
│   └─────────────────────────────────────────────────────┘  │
└────────────────────────────────────────────────────────────┘
```

### 3.2 核心数据结构设计

#### 3.2.1 SchedEntity（调度实体 - MVP 简化版）

从 `ProcessControlBlock` 中抽离调度相关字段，形成独立的调度实体：

```rust
/// 调度实体 - MVP 简化版
/// 设计原则：最小化字段，够用即可
pub struct SchedEntity {
    /// 调度状态（原子操作）
    state: AtomicU8,  // SchedState
    
    /// 当前/最近运行的 CPU（u32::MAX 表示未绑定）
    cpu: AtomicU32,
    
    /// 时间片（纳秒）
    slice: AtomicU64,
    
    /// 当前时间片已运行时间（纳秒）
    runtime: AtomicU64,
    
    /// 关联的 PCB（弱引用）
    pcb: Weak<ProcessControlBlock>,
}

impl SchedEntity {
    /// 默认时间片：10ms
    pub const DEFAULT_SLICE: u64 = 10_000_000;
    
    pub fn new(pcb: Weak<ProcessControlBlock>) -> Arc<Self> {
        Arc::new(Self {
            state: AtomicU8::new(SchedState::Runnable as u8),
            cpu: AtomicU32::new(u32::MAX),
            slice: AtomicU64::new(Self::DEFAULT_SLICE),
            runtime: AtomicU64::new(0),
            pcb,
        })
    }
    
    /// 重置时间片（用于重新入队时）
    pub fn reset_slice(&self) {
        self.runtime.store(0, Ordering::Relaxed);
    }
    
    /// 增加运行时间，返回是否超时
    pub fn charge_runtime(&self, delta: u64) -> bool {
        let new_runtime = self.runtime.fetch_add(delta, Ordering::Relaxed) + delta;
        new_runtime >= self.slice.load(Ordering::Relaxed)
    }
}

/// 调度状态（简化版）
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
}
```

#### 3.2.2 Scheduler Trait（调度器接口）

```rust
/// 调度器接口
pub trait Scheduler: Send + Sync {
    type Entity: SchedEntityOps;
    
    /// 将任务加入调度
    /// 返回应该被抢占的 CPU（如果有）
    fn enqueue(&self, entity: Arc<Self::Entity>, flags: EnqueueFlags) -> Option<CpuId>;
    
    /// 获取指定 CPU 的本地运行队列
    fn local_rq(&self, cpu: CpuId) -> &dyn LocalRunQueue<Entity = Self::Entity>;
    
    /// 获取指定 CPU 的本地运行队列（可变引用）
    fn local_rq_mut(&self, cpu: CpuId) -> &mut dyn LocalRunQueue<Entity = Self::Entity>;
    
    /// 选择最优 CPU
    fn select_cpu(&self, entity: &Self::Entity, flags: EnqueueFlags) -> CpuId;
}

/// 本地运行队列接口
pub trait LocalRunQueue: Send {
    type Entity: SchedEntityOps;
    
    /// 获取当前正在运行的实体
    fn current(&self) -> Option<Arc<Self::Entity>>;
    
    /// 更新当前任务状态
    /// 返回是否需要切换任务
    fn update_current(&mut self, flags: UpdateFlags) -> bool;
    
    /// 选择下一个任务
    fn pick_next(&mut self) -> Option<Arc<Self::Entity>>;
    
    /// 将当前任务从队列移除（睡眠/退出时）
    fn dequeue_current(&mut self);
    
    /// 队列中的任务数量
    fn nr_running(&self) -> usize;
}

/// 入队标志
bitflags! {
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
bitflags! {
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
```

#### 3.2.3 ClassScheduler（调度类实现）

```rust
/// 类调度器 - 多调度类聚合
pub struct ClassScheduler {
    /// 每 CPU 运行队列
    per_cpu_rqs: Box<[SpinLock<PerCpuClassRq, LocalIrqDisabled>]>,
    /// 上次选择的 CPU（负载均衡用）
    last_chosen_cpu: AtomicU32,
}

/// 每 CPU 调度类运行队列集合
struct PerCpuClassRq {
    /// CPU ID
    cpu: CpuId,
    /// RR 调度类（时间片轮转，MVP 核心）
    rr: RoundRobinClassRq,
    /// IDLE 调度类（最低优先级）
    idle: IdleClassRq,
    /// 当前运行的实体
    current: Option<Arc<SchedEntity>>,
    /// 当前任务开始执行的时间
    current_start: u64,
    /// 总运行任务数
    nr_running: usize,
}

/// 调度类运行队列接口
trait SchedClassRq: Send {
    /// 将实体加入队列
    fn enqueue(&mut self, entity: Arc<SchedEntity>, flags: EnqueueFlags);
    
    /// 选择下一个实体
    fn pick_next(&mut self) -> Option<Arc<SchedEntity>>;
    
    /// 更新当前实体
    /// 返回是否需要在该调度类内切换
    fn update_current(
        &mut self, 
        entity: &Arc<SchedEntity>,
        runtime: Duration,
        flags: UpdateFlags
    ) -> bool;
    
    /// 是否有就绪任务
    fn has_runnable(&self) -> bool;
    
    /// 队列长度
    fn len(&self) -> usize;
}
```

#### 3.2.4 RoundRobinClassRq（MVP 核心：时间片轮转）

```rust
/// 时间片轮转调度运行队列（MVP 核心实现）
pub struct RoundRobinClassRq {
    /// 就绪队列（简单的 VecDeque，O(1) 入队出队）
    queue: VecDeque<Arc<SchedEntity>>,
    /// 默认时间片（纳秒）
    default_slice: u64,
}

impl RoundRobinClassRq {
    /// 默认时间片：10ms
    const DEFAULT_TIME_SLICE: u64 = 10_000_000;
    
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            default_slice: Self::DEFAULT_TIME_SLICE,
        }
    }
}

impl SchedClassRq for RoundRobinClassRq {
    fn enqueue(&mut self, entity: Arc<SchedEntity>, _flags: EnqueueFlags) {
        // 简单地加入队尾
        self.queue.push_back(entity);
    }
    
    fn pick_next(&mut self) -> Option<Arc<SchedEntity>> {
        // 从队首取出
        self.queue.pop_front()
    }
    
    fn update_current(
        &mut self,
        entity: &Arc<SchedEntity>,
        runtime: Duration,
        flags: UpdateFlags,
    ) -> bool {
        // 时间片用完或主动让出，需要切换
        if flags.contains(UpdateFlags::YIELD) {
            return true;
        }
        
        if flags.contains(UpdateFlags::TICK) {
            // 检查时间片是否用完
            let slice = entity.rr_data().slice;
            if runtime.as_nanos() as u64 >= slice {
                return true;
            }
        }
        
        false
    }
    
    fn has_runnable(&self) -> bool {
        !self.queue.is_empty()
    }
    
    fn len(&self) -> usize {
        self.queue.len()
    }
}
```

#### 3.2.5 IdleClassRq（IDLE 调度类）

```rust
/// IDLE 调度类 - 每 CPU 一个 idle 任务
pub struct IdleClassRq {
    idle_entity: Option<Arc<SchedEntity>>,
}

impl SchedClassRq for IdleClassRq {
    fn enqueue(&mut self, entity: Arc<SchedEntity>, _flags: EnqueueFlags) {
        // idle 任务只有一个，直接设置
        self.idle_entity = Some(entity);
    }
    
    fn pick_next(&mut self) -> Option<Arc<SchedEntity>> {
        // 返回 idle 任务但不移除
        self.idle_entity.clone()
    }
    
    fn update_current(&mut self, _entity: &Arc<SchedEntity>, _runtime: Duration, _flags: UpdateFlags) -> bool {
        // idle 任务永不主动让出，只能被抢占
        false
    }
    
    fn has_runnable(&self) -> bool {
        self.idle_entity.is_some()
    }
    
    fn len(&self) -> usize {
        if self.idle_entity.is_some() { 1 } else { 0 }
    }
}
```

#### 3.2.6 调度决策逻辑

```rust
impl PerCpuClassRq {
    /// 选择下一个任务（按优先级：RR -> IDLE）
    fn pick_next_entity(&mut self) -> Option<Arc<SchedEntity>> {
        // 1. 优先从 RR 队列选择
        if let Some(entity) = self.rr.pick_next() {
            return Some(entity);
        }
        
        // 2. 没有普通任务，返回 idle
        self.idle.pick_next()
    }
    
    /// 将任务入队到对应的调度类
    fn enqueue_entity(&mut self, entity: Arc<SchedEntity>, flags: EnqueueFlags) {
        match entity.policy() {
            SchedPolicy::RoundRobin | SchedPolicy::Normal => {
                self.rr.enqueue(entity, flags);
            }
            SchedPolicy::Idle => {
                self.idle.enqueue(entity, flags);
            }
        }
        self.nr_running += 1;
    }
}
```

### 3.3 调度核心流程

#### 3.3.1 任务入队流程

```
                     ┌─────────────────┐
                     │  sched_enqueue  │
                     └────────┬────────┘
                              │
                              ▼
                     ┌─────────────────┐
                     │  select_cpu()   │
                     │  选择目标 CPU    │
                     └────────┬────────┘
                              │
                              ▼
              ┌───────────────────────────────┐
              │  AtomicCpuId::set_if_none()   │
              │  原子设置 CPU 绑定             │
              └───────────────┬───────────────┘
                              │
                     ┌────────┴────────┐
                     │ 获取目标 CPU 的  │
                     │ PerCpuClassRq   │
                     │   (with lock)   │
                     └────────┬────────┘
                              │
                              ▼
              ┌───────────────────────────────┐
              │  根据 policy 选择调度类        │
              │  调用对应 SchedClassRq.enqueue │
              └───────────────┬───────────────┘
                              │
                              ▼
              ┌───────────────────────────────┐
              │  检查是否需要抢占当前任务       │
              │  返回应抢占的 CPU ID           │
              └───────────────────────────────┘
```

#### 3.3.2 调度决策流程

```
                     ┌─────────────────┐
                     │   schedule()    │
                     │  (调度入口)      │
                     └────────┬────────┘
                              │
                              ▼
              ┌───────────────────────────────┐
              │    获取当前 CPU 的 LocalRq    │
              │    update_current(flags)     │
              └───────────────┬───────────────┘
                              │
                    ┌─────────┴─────────┐
                    │   需要切换任务？   │
                    └─────────┬─────────┘
                      Yes     │     No
                    ┌─────────┴─────────┐
                    │                   │
                    ▼                   ▼
         ┌──────────────────┐    ┌────────────┐
         │  若需睡眠/退出    │    │  继续运行   │
         │  dequeue_current │    │  当前任务   │
         └────────┬─────────┘    └────────────┘
                  │
                  ▼
         ┌──────────────────┐
         │   pick_next()    │
         │  按优先级选择:    │
         │   RR → IDLE      │
         └────────┬─────────┘
                  │
                  ▼
         ┌──────────────────┐
         │ context_switch() │
         │   上下文切换      │
         └──────────────────┘
```

#### 3.3.3 阻塞/唤醒流程

```
[阻塞]
                     ┌─────────────────┐
                     │  park_current   │
                     └────────┬────────┘
                              │
                              ▼
              ┌───────────────────────────────┐
              │  update_current(WAIT)         │
              │  dequeue_current()            │
              └───────────────┬───────────────┘
                              │
                              ▼
              ┌───────────────────────────────┐
              │  entity.state = Interruptible │
              │  pick_next() + switch         │
              └───────────────────────────────┘

[唤醒]
                     ┌─────────────────┐
                     │  unpark_target  │
                     └────────┬────────┘
                              │
                              ▼
              ┌───────────────────────────────┐
              │  entity.state = Runnable      │
              │  scheduler.enqueue(WAKE)     │
              └───────────────┬───────────────┘
                              │
                              ▼
              ┌───────────────────────────────┐
              │  若返回 CPU ID，设置抢占标志   │
              │  send_resched_ipi() 如需要    │
              └───────────────────────────────┘
```

### 3.4 多核负载均衡设计

#### 3.4.1 CPU 选择策略

```rust
impl ClassScheduler {
    fn select_cpu(&self, entity: &SchedEntity, flags: EnqueueFlags) -> CpuId {
        let affinity = entity.cpu_affinity();
        
        // 1. 优先使用上次运行的 CPU（缓存亲和性）
        if let Some(last_cpu) = entity.cpu.get() {
            if affinity.contains(last_cpu) {
                let rq = self.per_cpu_rqs[last_cpu.as_usize()].lock();
                if rq.nr_running < MIGRATE_THRESHOLD {
                    return last_cpu;
                }
            }
        }
        
        // 2. 对于新任务，使用简单的负载均衡
        if flags.contains(EnqueueFlags::SPAWN) {
            return self.select_least_loaded_cpu(affinity);
        }
        
        // 3. 唤醒时，优先使用当前 CPU 或上次 CPU
        if flags.contains(EnqueueFlags::WAKE) {
            let current_cpu = current_cpu_id();
            if affinity.contains(current_cpu) {
                return current_cpu;
            }
        }
        
        // 4. 默认选择负载最低的 CPU
        self.select_least_loaded_cpu(affinity)
    }
    
    fn select_least_loaded_cpu(&self, affinity: CpuSet) -> CpuId {
        let mut min_load = usize::MAX;
        let mut selected = CpuId(0);
        
        // 从 last_chosen_cpu 开始轮询，避免总是选择同一个 CPU
        let start = self.last_chosen_cpu.load(Ordering::Relaxed);
        let num_cpus = self.per_cpu_rqs.len();
        
        for i in 0..num_cpus {
            let cpu = CpuId((start as usize + i) % num_cpus);
            if !affinity.contains(cpu) {
                continue;
            }
            
            let rq = self.per_cpu_rqs[cpu.as_usize()].lock();
            if rq.nr_running < min_load {
                min_load = rq.nr_running;
                selected = cpu;
            }
        }
        
        self.last_chosen_cpu.store(selected.0, Ordering::Relaxed);
        selected
    }
}
```

#### 3.4.2 周期性负载均衡（后续扩展）

```rust
/// 负载均衡器（可选，后续实现）
pub struct LoadBalancer {
    /// 负载均衡间隔
    interval: Duration,
    /// 迁移阈值
    migrate_threshold: usize,
}

impl LoadBalancer {
    /// 周期性检查并迁移任务
    pub fn balance(&self, scheduler: &ClassScheduler) {
        let (most_loaded, least_loaded) = self.find_imbalanced_cpus(scheduler);
        
        if let Some((from, to)) = most_loaded.zip(least_loaded) {
            if let Some(entity) = self.select_task_to_migrate(scheduler, from) {
                self.migrate_task(scheduler, entity, from, to);
            }
        }
    }
}
```

### 3.5 与其他子系统的接口

#### 3.5.1 与进程管理的接口

```rust
// ProcessControlBlock 修改
impl ProcessControlBlock {
    /// 获取调度实体（新增）
    pub fn sched_entity(&self) -> &Arc<SchedEntity> {
        &self.sched_entity
    }
    
    // 移除原有的 sched_info() 方法中的调度相关字段
    // 保留：on_cpu, state 等基本信息
}

// ProcessManager 接口调整
impl ProcessManager {
    /// 唤醒进程（调用新调度器接口）
    pub fn wakeup(pcb: &Arc<ProcessControlBlock>) -> Result<(), SystemError> {
        let entity = pcb.sched_entity();
        scheduler::wakeup(entity)
    }
    
    /// 当前进程睡眠
    pub fn mark_sleep(interruptible: bool) -> Result<(), SystemError> {
        let pcb = Self::current_pcb();
        let entity = pcb.sched_entity();
        entity.set_state(if interruptible {
            SchedState::Interruptible
        } else {
            SchedState::Uninterruptible
        });
        Ok(())
    }
}
```

#### 3.5.2 与同步原语的接口

```rust
// WaitQueue 修改
impl WaitQueue {
    pub fn sleep(&self) -> Result<(), SystemError> {
        let pcb = ProcessManager::current_pcb();
        let entity = pcb.sched_entity();
        
        // 加入等待队列
        self.add_waiter(pcb.clone());
        
        // 标记睡眠并调度
        scheduler::park_current(|| {
            // 检查是否已被唤醒
            !self.contains(&pcb)
        })
    }
    
    pub fn wakeup(&self, state: Option<ProcessState>) -> bool {
        if let Some(pcb) = self.pop_front(state) {
            scheduler::unpark(&pcb.sched_entity())
        } else {
            false
        }
    }
}
```

#### 3.5.3 与时钟中断的接口

```rust
// 时钟 tick 处理
pub fn scheduler_tick() {
    let cpu = current_cpu_id();
    let scheduler = get_scheduler();
    
    scheduler.local_rq_with_mut(cpu, |rq| {
        // 更新当前任务
        if rq.update_current(UpdateFlags::TICK) {
            // 需要抢占
            set_need_preempt();
        }
    });
}
```

---

## 4. 迭代计划（MVP 优先）

### 4.0 MVP 策略

**核心思路**：最小化改动，快速验证新框架

- **MVP 调度策略**：RR（时间片轮转）+ IDLE，不实现 CFS
- **与旧代码共存**：通过 feature flag 切换，新旧调度器可并行存在
- **渐进式替换**：先让新框架 work，再逐步迁移

### 4.1 Phase 1：MVP 核心框架（3-5 天）

**目标**：实现最小可运行的新调度框架，能够启动到 shell

**Day 1-2：核心数据结构**

- [ ] 创建 `kernel/src/sched_new/` 目录
- [ ] 实现 `SchedEntity`（简化版，仅包含必要字段）
  ```rust
  pub struct SchedEntity {
      state: AtomicU8,           // SchedState
      cpu: AtomicU32,            // 当前 CPU
      slice: AtomicU64,          // 时间片
      runtime: AtomicU64,        // 已运行时间
      pcb: Weak<ProcessControlBlock>,
  }
  ```
- [ ] 实现 `RoundRobinClassRq`（VecDeque 实现）
- [ ] 实现 `IdleClassRq`

**Day 3-4：调度器骨架**

- [x] 实现 `PerCpuClassRq`
- [x] 实现 `ClassScheduler`
- [x] 实现核心调度函数：
  - [x] `sched_enqueue()` - 入队
  - [x] `sched_dequeue()` - 出队
  - [x] `sched_pick_next()` - 选择下一个
  - [x] `sched_tick()` - 时钟 tick 处理

**Day 5：集成与验证**

- [x] 在 `ProcessControlBlock` 中添加 `sched_entity` 字段（与旧字段并存）
- [x] 实现适配层，将新调度器接入现有流程：
  ```rust
  // 新增 feature flag
  #[cfg(feature = "sched_new")]
  pub fn schedule(mode: SchedMode) {
      sched_new::schedule(mode);
  }
  
  #[cfg(not(feature = "sched_new"))]
  pub fn schedule(mode: SchedMode) {
      __schedule(mode);  // 旧实现
  }
  ```
- [x] 适配 `idle.rs` - 初始化 idle 进程的 SchedEntity
- [x] 启动测试，验证能进入 shell

**交付物**：
- 默认启用 sched_new 特性，编译可启动到 shell
- 基本的进程切换工作

### 4.2 Phase 2：关键路径适配（2-3 天）

**目标**：适配核心的睡眠/唤醒路径

**任务列表**：

- [x] 适配 `ProcessManager::wakeup()`
  - 已在 `process/mod.rs` 中通过 `#[cfg(feature = "sched_new")]` 条件编译实现
  - 调用 `sched_new::wakeup(pcb.sched_entity())` 入队

- [x] 适配 `ProcessManager::mark_sleep()`
  - 已在 `process/mod.rs` 中适配
  - 同步更新 SchedEntity 的状态（interruptible/uninterruptible）

- [x] 适配 `WaitQueue` 的关键方法：
  - [x] `sleep()` - 通过 ProcessManager::mark_sleep() + schedule() 间接适配
  - [x] `wakeup()` - 通过 ProcessManager::wakeup() 间接适配
  - [x] `wakeup_all()` - 通过 ProcessManager::wakeup() 间接适配

- [x] 适配 fork 流程：
  - [x] `fork.rs` 中通过 `ProcessManager::wakeup()` 唤醒新进程
  - [x] 新进程的 `SchedEntity` 在 `ProcessControlBlock::new()` 中初始化

**交付物**：
- 基本的睡眠/唤醒工作
- 简单的多进程程序可以运行

### 4.3 Phase 3：完善与稳定（2-3 天）

**目标**：修复问题，确保稳定

**任务列表**：

- [x] 适配 `Mutex`
  - Mutex 通过 `ProcessManager::mark_sleep()` + `schedule()` 实现阻塞
  - 通过 `ProcessManager::wakeup()` 实现唤醒
  - 两者都已在 Phase 2 中适配新调度器
- [x] 适配 `Completion`
  - Completion 通过 `WaitQueue` 实现，间接适配
- [ ] 修复发现的 bug
- [ ] 基本的压力测试

**交付物**：
- MVP 稳定运行
- 可以运行常见的用户态程序

---

### 4.4 后续阶段（MVP 后）

以下阶段在 MVP 验证成功后进行：

#### Phase 4：清理与优化（1 周）

- [ ] 移除旧调度器代码（或保留为备选）
- [ ] 清理 `ProcessControlBlock` 中的冗余字段
- [ ] 性能优化

#### Phase 5：CFS 实现（可选，2 周）

- [ ] 实现 `FairClassRq`
- [ ] 支持 nice 值和优先级

#### Phase 6：多核负载均衡（2 周）

- [ ] 智能 CPU 选择
- [ ] 任务迁移
- [ ] 周期性负载均衡

---

## 5. 风险与缓解措施

### 5.1 技术风险

| 风险 | 可能性 | 影响 | 缓解措施 |
|------|--------|------|----------|
| 新调度器导致系统无法启动 | 中 | 高 | 使用 feature flag，默认使用旧调度器，可随时切换回去 |
| 睡眠/唤醒适配不完整 | 中 | 高 | MVP 阶段只适配核心路径，其他保持旧实现 |
| 性能退化 | 低 | 中 | RR 调度器非常简单，性能开销小 |

### 5.2 MVP 风险控制

1. **保留旧实现**：通过 `#[cfg(feature = "sched_new")]` 条件编译，新旧代码并存
2. **最小改动原则**：MVP 阶段尽量不修改旧代码，只新增代码
3. **快速回滚**：如果新调度器有问题，只需移除 feature flag 即可回滚

---

## 6. 成功标准

### 6.1 MVP 成功标准

- [ ] 使用 `--features sched_new` 编译，系统能启动到 shell
- [ ] 基本的进程创建、执行、退出正常
- [ ] 时钟中断能正确触发调度
- [ ] sleep/wakeup 基本工作
- [ ] 无明显的死锁或崩溃

### 6.2 后续标准（MVP 后）

- [ ] 所有现有测试通过
- [ ] 多核调度正常
- [ ] 性能不低于旧实现

---

## 7. 参考资源

### 7.1 Asterinas 调度代码参考

- 调度抽象与注入：`ostd/src/task/scheduler/mod.rs`
- Task 抽象：`ostd/src/task/mod.rs`
- 调度类框架：`kernel/src/sched/sched_class/mod.rs`

### 7.2 文档参考

- [Asterinas 调度子系统架构分析](./asterinas_scheduler_architecture.md)

---

## 8. MVP TODO List（开发跟踪）

### Phase 1 TODO（3-5 天）

**Day 1-2：数据结构**
- [x] 创建 `kernel/src/sched_new/mod.rs`
- [x] 创建 `kernel/src/sched_new/entity.rs` - SchedEntity
- [x] 创建 `kernel/src/sched_new/rr.rs` - RoundRobinClassRq
- [x] 创建 `kernel/src/sched_new/idle.rs` - IdleClassRq

**Day 3-4：调度器核心**
- [x] 创建 `kernel/src/sched_new/class_rq.rs` - PerCpuClassRq
- [x] 创建 `kernel/src/sched_new/scheduler.rs` - ClassScheduler
- [x] 实现 `sched_enqueue()`
- [x] 实现 `sched_pick_next()`
- [x] 实现 `schedule()` 入口

**Day 5：集成**
- [x] 添加 feature flag `sched_new`
- [x] 在 PCB 中添加 `sched_entity` 字段
- [x] 适配 idle 进程初始化
- [x] 适配 `scheduler_tick()`
- [x] 适配 `schedule()` / `__schedule()` 入口
- [x] **Phase 1 完成** - 核心框架已就绪

### Phase 2 TODO（2-3 天）

- [x] 适配 `ProcessManager::wakeup()`
- [x] 适配 `ProcessManager::mark_sleep()`
- [x] 适配 `WaitQueue::sleep()`
- [x] 适配 `WaitQueue::wakeup()`
- [x] 适配 fork 流程
- [ ] **测试 - 多进程程序**

### Phase 3 TODO（2-3 天）

- [x] 适配 `Mutex`
- [x] 适配 `Completion`
- [ ] Bug 修复
- [ ] **测试 - 稳定性验证**

---

## 9. 快速开始指南

### 9.1 编译新调度器

```bash
# 使用新调度器编译
make FEATURES=sched_new

# 或在 Cargo.toml 中临时启用
[features]
sched_new = []
```

### 9.2 目录结构

```
kernel/src/sched_new/
├── mod.rs          # 模块入口，导出公共接口
├── entity.rs       # SchedEntity 定义
├── rr.rs           # RoundRobin 调度类
├── idle.rs         # Idle 调度类
├── class_rq.rs     # PerCpuClassRq
└── scheduler.rs    # ClassScheduler 和全局调度函数
```

### 9.3 关键接口

```rust
// 新调度器的公共接口
pub mod sched_new {
    /// 将任务加入调度
    pub fn enqueue(entity: &Arc<SchedEntity>, flags: EnqueueFlags);
    
    /// 唤醒任务
    pub fn wakeup(entity: &Arc<SchedEntity>) -> Result<(), SystemError>;
    
    /// 当前任务睡眠
    pub fn park_current();
    
    /// 调度入口
    pub fn schedule(mode: SchedMode);
    
    /// 时钟 tick
    pub fn tick();
}
```

---

*文档版本：v1.1 (MVP 优化版)*  
*创建日期：2025-12-03*  
*最后更新：2025-12-03*
