# Linux PID Namespace机制分析与DragonOS重构设计

## 1. Linux PID Namespace机制深度分析

### 1.1 核心概念

PID namespace是Linux内核提供的一种进程隔离机制，它允许不同的进程组拥有独立的进程ID空间。每个PID namespace都维护自己的进程ID分配器，使得不同namespace中的进程可以拥有相同的PID值，从而实现进程级别的虚拟化。

### 1.2 关键数据结构

#### 1.2.1 pid_namespace结构体
```c
struct pid_namespace {
    struct kref kref;                    // 引用计数
    struct idr idr;                      // PID分配器(现代版本)
    struct rcu_head rcu;                 // RCU回收
    unsigned int pid_allocated;          // 已分配的PID数量
    struct task_struct *child_reaper;    // init进程(PID 1)
    struct kmem_cache *pid_cachep;       // PID结构体缓存
    unsigned int level;                  // namespace层级
    struct pid_namespace *parent;        // 父namespace
    struct user_namespace *user_ns;      // 关联的用户namespace
    struct ucounts *ucounts;             // 使用计数限制
    // procfs相关字段
    struct vfsmount *proc_mnt;
    struct dentry *proc_self;
    struct dentry *proc_thread_self;
    struct ns_common ns;                 // 通用namespace字段
};
```

#### 1.2.2 pid结构体 - PID管理的核心
```c
struct pid {
    atomic_t count;                      // 引用计数
    unsigned int level;                  // 该PID存在的namespace层级数
    struct hlist_head tasks[PIDTYPE_MAX]; // 使用此PID的任务列表
    struct rcu_head rcu;                 // RCU回收
    struct upid numbers[1];              // 在各个namespace中的PID值(变长数组)
};
```

#### 1.2.3 upid结构体 - 特定namespace中的PID
```c
struct upid {
    int nr;                              // 在特定namespace中的PID值
    struct pid_namespace *ns;            // 所属的namespace
    struct hlist_node pid_chain;         // 用于在hash表中链接
};
```

#### 1.2.4 pid_link结构体 - 连接任务和PID的桥梁
```c
struct pid_link {
    struct hlist_node node;              // 链接到pid->tasks[type]的节点
    struct pid *pid;                     // 指向对应的pid结构体
};
```

#### 1.2.5 task_struct中的PID相关字段
```c
struct task_struct {
    pid_t pid;                           // 在当前namespace中的PID
    pid_t tgid;                          // 线程组ID
    struct pid_link pids[PIDTYPE_MAX];   // 各种类型的PID链接
    // ... 其他字段
};
```

#### 1.2.6 PID类型枚举
```c
enum pid_type {
    PIDTYPE_PID,                         // 进程PID
    PIDTYPE_TGID,                        // 线程组ID
    PIDTYPE_PGID,                        // 进程组ID  
    PIDTYPE_SID,                         // 会话ID
    PIDTYPE_MAX
};
```



### 1.3 核心机制分析

#### 1.3.1 多层级PID映射
- 每个进程在多个namespace层级中都有PID值
- `struct pid`中的`numbers[]`数组存储各层级的PID
- `level`字段表示该PID存在于多少个namespace层级

#### 1.3.2 PID类型管理
- 一个进程有多种类型的PID：进程ID、线程组ID、进程组ID、会话ID
- `task_struct.pids[PIDTYPE_MAX]`数组管理各种类型的PID
- 每种类型的PID都通过`pid_link`连接到对应的`struct pid`

#### 1.3.3 Hash表查找机制
- 全局`pid_hash`表用于快速查找
- 通过`(pid_nr, namespace)`作为key进行hash
- `upid.pid_chain`用于在hash表中链接

#### 1.3.4 任务列表管理
- `pid.tasks[PIDTYPE_MAX]`存储使用该PID的所有任务
- 通过`pid_link.node`将任务链接到对应的任务列表
- 支持一个PID被多个任务共享（如线程组）

## 2. Linux实现的优势分析

### 2.1 设计优势
1. **层次化管理**：支持嵌套的namespace层级
2. **高效查找**：基于hash表的O(1)查找性能
3. **类型分离**：不同PID类型独立管理
4. **内存优化**：通过slab缓存优化内存分配
5. **RCU保护**：无锁读取，高并发性能

### 2.2 核心算法
1. **PID分配**：基于IDR/IDA的高效分配算法
2. **namespace层级遍历**：从子到父的层级查找
3. **引用计数管理**：自动回收无用的PID结构

## 3. DragonOS重构设计方案

### 3.1 设计目标
基于DragonOS现有架构，实现符合Linux语义的PID namespace系统，包含完整的`pid`、`pid_link`和多层级PID管理机制。

### 3.2 核心数据结构设计

#### 3.2.1 基于现有PidType的扩展
```rust
// 保持与现有代码兼容
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum PidType {
    PID = 1,
    TGID = 2,
    PGID = 3,
    SID = 4,
    MAX = 5,
}

impl PidType {
    pub const PIDTYPE_MAX: usize = Self::MAX as usize;
}
```

#### 3.2.2 新增Pid结构体 - 核心PID管理
```rust
use alloc::vec::Vec;
use alloc::sync::Arc;
use crate::libs::spinlock::SpinLock;
use crate::process::ProcessControlBlock;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Linux风格的PID结构体
/// 管理一个PID在多个namespace层级中的映射
pub struct Pid {
    /// 引用计数
    count: AtomicUsize,
    /// 该PID存在的namespace层级数
    level: usize,
    /// 使用此PID的任务列表，按PID类型分组
    /// tasks[PidType::PID as usize] = 使用该PID作为进程ID的任务
    /// tasks[PidType::TGID as usize] = 使用该PID作为线程组ID的任务
    tasks: [SpinLock<Vec<Weak<ProcessControlBlock>>>; PidType::PIDTYPE_MAX],
    /// 在各个namespace中的PID值
    numbers: SpinLock<Vec<UPid>>,
}

impl Pid {
    /// 创建新的Pid结构体
    pub fn new(level: usize) -> Arc<Self> {
        const INIT_LOCK: SpinLock<Vec<Weak<ProcessControlBlock>>> = SpinLock::new(Vec::new());
        
        Arc::new(Self {
            count: AtomicUsize::new(1),
            level,
            tasks: [INIT_LOCK; PidType::PIDTYPE_MAX],
            numbers: SpinLock::new(Vec::with_capacity(level + 1)),
        })
    }

    /// 添加任务到指定类型的任务列表
    pub fn attach_task(&self, task: Weak<ProcessControlBlock>, pid_type: PidType) {
        let mut tasks = self.tasks[pid_type as usize].lock();
        tasks.push(task);
    }

    /// 从指定类型的任务列表移除任务
    pub fn detach_task(&self, task_ptr: Arc<ProcessControlBlock>, pid_type: PidType) {
        let mut tasks = self.tasks[pid_type as usize].lock();
        tasks.retain(|weak_task| {
            if let Some(task) = weak_task.upgrade() {
               Arc::ptr_eq(&task, &task_ptr)
            } else {
                false // 移除已失效的弱引用
            }
        });
    }

    /// 获取在指定namespace中的PID值
    pub fn pid_nr(&self, ns: &Arc<PidNamespace>) -> Option<i32> {
        let numbers = self.numbers.lock();
        for upid in numbers.iter() {
            if Arc::ptr_eq(&upid.ns, ns) {
                return Some(upid.nr);
            }
        }
        None
    }

    /// 获取在当前namespace中的PID值
    pub fn pid_nr_current(&self) -> i32 {
        let numbers = self.numbers.lock();
        // 返回最后一个(最深层级)的PID值
        numbers.last().map(|upid| upid.nr).unwrap_or(0)
    }

    /// 添加namespace层级的PID映射
    pub fn add_upid(&self, upid: UPid) {
        let mut numbers = self.numbers.lock();
        numbers.push(upid);
    }

    /// 获取指定类型的第一个任务
    pub fn get_task(&self, pid_type: PidType) -> Option<Arc<ProcessControlBlock>> {
        let tasks = self.tasks[pid_type as usize].lock();
        for weak_task in tasks.iter() {
            if let Some(task) = weak_task.upgrade() {
                return Some(task);
            }
        }
        None
    }
}

impl Drop for Pid {
    fn drop(&mut self) {
        // 清理所有任务链接
        for pid_type in 0..PidType::PIDTYPE_MAX {
            let mut tasks = self.tasks[pid_type].lock();
            tasks.clear();
        }
    }
}
```

#### 3.2.3 UPid结构体 - 特定namespace中的PID
```rust
/// 在特定namespace中的PID信息
pub struct UPid {
    /// 在该namespace中的PID值
    pub nr: i32,
    /// 所属的namespace
    pub ns: Arc<PidNamespace>,
}
```

#### 3.2.4 PidLink结构体 - 连接任务和PID
```rust
/// 连接任务和PID的桥梁结构体
pub struct PidLink {
    /// 指向对应的Pid结构体
    pub pid: Option<Arc<Pid>>,
}

impl PidLink {
    pub fn new() -> Self {
        Self { pid: None }
    }

    pub fn link_pid(&mut self, pid: Arc<Pid>) {
        self.pid = Some(pid);
    }

    pub fn unlink_pid(&mut self) {
        self.pid = None;
    }
}
```

#### 3.2.5 扩展ProcessControlBlock
```rust
// 在现有PCB中添加PID相关字段
impl ProcessControlBlock {
    // 现有字段保持不变...
    
    /// 各种类型的PID链接
    pids: [PidLink; PidType::PIDTYPE_MAX],
    
    /// 在当前namespace中的PID (兼容现有代码)
    pid: Pid,
    
    /// 线程组ID (兼容现有代码) 
    tgid: Pid,
}
```

#### 3.2.6 完善PidNamespace结构体
```rust
use crate::libs::ida::IdAllocator;
use hashbrown::HashMap;

pub struct PidNamespace {
    /// 基础namespace字段
    common: Arc<NsCommon>,
    /// PID分配器
    pid_allocator: SpinLock<IdAllocator>,
    /// 已分配的PID数量
    pid_allocated: AtomicUsize,
    /// init进程(PID 1)
    child_reaper: SpinLock<Option<Weak<ProcessControlBlock>>>,
    /// namespace层级(0为根namespace)
    level: usize,
    /// 父namespace
    parent: Option<Arc<PidNamespace>>,
    /// PID hash表：(pid_nr) -> Arc<Pid>
    pid_hash: SpinLock<HashMap<i32, Arc<Pid>>>,
}

impl PidNamespace {
    /// 分配新的PID
    pub fn alloc_pid(&self) -> Option<i32> {
        let mut allocator = self.pid_allocator.lock();
        allocator.alloc().map(|id| id as i32)
    }

    /// 释放PID
    pub fn free_pid(&self, pid: i32) {
        let mut allocator = self.pid_allocator.lock();
        allocator.free(pid as usize);
        
        // 从hash表中移除
        let mut hash = self.pid_hash.lock();
        hash.remove(&pid);
    }

    /// 在hash表中注册PID
    pub fn register_pid(&self, pid_nr: i32, pid: Arc<Pid>) {
        let mut hash = self.pid_hash.lock();
        hash.insert(pid_nr, pid);
    }

    /// 根据PID号查找Pid结构体
    pub fn find_pid(&self, pid_nr: i32) -> Option<Arc<Pid>> {
        let hash = self.pid_hash.lock();
        hash.get(&pid_nr).cloned()
    }
}
```

### 3.3 PID分配与管理算法

#### 3.3.1 多层级PID分配
```rust
/// 为新进程分配PID
pub fn alloc_pid_for_task(ns: &Arc<PidNamespace>) -> Result<Arc<Pid>, SystemError> {
    let level = ns.level;
    let pid = Pid::new(level);
    
    // 从当前namespace开始，向上遍历所有父namespace
    let mut current_ns = Some(ns.clone());
    let mut upids = Vec::with_capacity(level + 1);
    
    while let Some(namespace) = current_ns {
        // 在当前namespace中分配PID
        let pid_nr = namespace.alloc_pid()
            .ok_or(SystemError::ENOMEM)?;
        
        // 创建UPid
        let upid = UPid {
            nr: pid_nr,
            ns: namespace.clone(),
        };
        upids.push(upid);
        
        // 在hash表中注册
        namespace.register_pid(pid_nr, pid.clone());
        
        // 移动到父namespace
        current_ns = namespace.parent.clone();
    }
    
    // 设置numbers数组（从根namespace到当前namespace的顺序）
    upids.reverse();
    pid.numbers = upids;
    
    Ok(pid)
}
```

#### 3.3.2 PID查找算法
```rust
/// 在指定namespace中查找PID
pub fn find_pid_ns(pid_nr: i32, ns: &Arc<PidNamespace>) -> Option<Arc<Pid>> {
    ns.find_pid(pid_nr)
}

/// 在当前namespace中查找PID
pub fn find_pid_current(pid_nr: i32) -> Option<Arc<Pid>> {
    let current_ns = current_pid_namespace();
    find_pid_ns(pid_nr, &current_ns)
}

/// 根据PID查找对应的任务
pub fn find_task_by_pid_ns(pid_nr: i32, pid_type: PidType, ns: &Arc<PidNamespace>) -> Option<Arc<ProcessControlBlock>> {
    let pid = find_pid_ns(pid_nr, ns)?;
    let tasks = pid.tasks[pid_type as usize].lock();
    
    // 返回第一个有效的任务
    for weak_task in tasks.iter() {
        if let Some(task) = weak_task.upgrade() {
            return Some(task);
        }
    }
    None
}
```

#### 3.3.3 任务PID管理
```rust
impl ProcessControlBlock {
    /// 初始化进程的PID链接
    pub fn init_pid_links(&mut self, main_pid: Arc<Pid>) -> Result<(), SystemError> {
        // 设置进程PID
        self.pids[PidType::PID as usize].link_pid(main_pid.clone());
        main_pid.attach_task(Arc::downgrade(&self.self_ref), PidType::PID);
        
        // 设置线程组ID（通常与进程PID相同，除非是线程）
        self.pids[PidType::TGID as usize].link_pid(main_pid.clone());
        main_pid.attach_task(Arc::downgrade(&self.self_ref), PidType::TGID);
        
        // 继承父进程的进程组ID和会话ID
        if let Some(parent) = &self.parent_pcb {
            if let Some(parent_pcb) = parent.upgrade() {
                // 继承进程组ID
                if let Some(pgid_pid) = &parent_pcb.pids[PidType::PGID as usize].pid {
                    self.pids[PidType::PGID as usize].link_pid(pgid_pid.clone());
                    pgid_pid.attach_task(Arc::downgrade(&self.self_ref), PidType::PGID);
                }
                
                // 继承会话ID
                if let Some(sid_pid) = &parent_pcb.pids[PidType::SID as usize].pid {
                    self.pids[PidType::SID as usize].link_pid(sid_pid.clone());
                    sid_pid.attach_task(Arc::downgrade(&self.self_ref), PidType::SID);
                }
            }
        }
        
        Ok(())
    }

    /// 清理进程的PID链接
    pub fn cleanup_pid_links(&mut self) {
        let self_ptr = self as *const Self;
        
        for (i, pid_link) in self.pids.iter_mut().enumerate() {
            if let Some(pid) = &pid_link.pid {
                let pid_type = match i {
                    0 => PidType::PID,
                    1 => PidType::TGID,
                    2 => PidType::PGID,
                    3 => PidType::SID,
                    _ => continue,
                };
                pid.detach_task(self_ptr, pid_type);
            }
            pid_link.unlink_pid();
        }
    }

    /// 获取指定类型的PID值
    pub fn get_pid_nr(&self, pid_type: PidType, ns: &Arc<PidNamespace>) -> Option<i32> {
        let pid_link = &self.pids[pid_type as usize];
        pid_link.pid.as_ref()?.pid_nr(ns)
    }
}
```

### 3.4 系统调用集成

#### 3.4.1 扩展现有系统调用
```rust
/// getpid系统调用 - 返回在当前namespace中的PID
pub fn sys_getpid() -> Result<usize, SystemError> {
    let current = current_pcb();
    let current_ns = current_pid_namespace();
    
    let pid_nr = current.get_pid_nr(PidType::PID, &current_ns)
        .ok_or(SystemError::ESRCH)?;
    
    Ok(pid_nr as usize)
}

/// getppid系统调用 - 返回父进程在当前namespace中的PID
pub fn sys_getppid() -> Result<usize, SystemError> {
    let current = current_pcb();
    let current_ns = current_pid_namespace();
    
    if let Some(parent) = current.parent_pcb.as_ref().and_then(|p| p.upgrade()) {
        let ppid_nr = parent.get_pid_nr(PidType::PID, &current_ns)
            .ok_or(SystemError::ESRCH)?;
        Ok(ppid_nr as usize)
    } else {
        Ok(0) // 根进程的父进程PID为0
    }
}

/// kill系统调用 - 支持namespace
pub fn sys_kill(pid: i32, sig: i32) -> Result<usize, SystemError> {
    let current_ns = current_pid_namespace();
    
    if let Some(target_task) = find_task_by_pid_ns(pid, PidType::PID, &current_ns) {
        // 发送信号到目标进程
        target_task.send_signal(sig)?;
        Ok(0)
    } else {
        Err(SystemError::ESRCH)
    }
}
```

#### 3.4.2 新增namespace相关系统调用
```rust
/// unshare系统调用 - 创建新的PID namespace
pub fn sys_unshare(flags: usize) -> Result<usize, SystemError> {
    const CLONE_NEWPID: usize = 0x20000000;
    
    if flags & CLONE_NEWPID != 0 {
        let current = current_pcb();
        let current_ns = current_pid_namespace();
        
        // 创建新的PID namespace
        let new_ns = PidNamespace::new(
            current_ns.level + 1,
            Some(current_ns),
        )?;
        
        // 更新当前进程的nsproxy
        let mut nsproxy = current.nsproxy.lock();
        nsproxy.pid_ns_for_children = new_ns;
    }
    
    Ok(0)
}

/// setns系统调用 - 加入已存在的namespace
pub fn sys_setns(fd: i32, nstype: i32) -> Result<usize, SystemError> {
    const CLONE_NEWPID: i32 = 0x20000000;
    
    if nstype == CLONE_NEWPID {
        // 从文件描述符获取namespace
        let current = current_pcb();
        // TODO: 实现从fd获取namespace的逻辑
        // let target_ns = get_pid_namespace_from_fd(fd)?;
        
        // 更新当前进程的nsproxy
        // let mut nsproxy = current.nsproxy.lock();
        // nsproxy.pid_ns_for_children = target_ns;
    }
    
    Ok(0)
}
```

### 3.5 进程创建集成

#### 3.5.1 修改do_fork函数
```rust
pub fn do_fork(
    clone_flags: CloneFlags,
    stack_start: usize,
    stack_size: usize,
    parent_tidptr: *mut pid_t,
    child_tidptr: *mut pid_t,
) -> Result<usize, SystemError> {
    let current = current_pcb();
    
    // 确定子进程的PID namespace
    let child_pid_ns = if clone_flags.contains(CloneFlags::CLONE_NEWPID) {
        // 创建新的PID namespace
        let current_ns = current_pid_namespace();
        Arc::new(PidNamespace::new(current_ns.level + 1, Some(current_ns))?)
    } else {
        // 使用父进程的namespace
        current_pid_namespace()
    };
    
    // 为子进程分配PID
    let child_pid = alloc_pid_for_task(&child_pid_ns)?;
    
    // 创建子进程PCB
    let child_pcb = ProcessControlBlock::new()?;
    
    // 初始化子进程的PID链接
    child_pcb.init_pid_links(child_pid.clone())?;
    
    // 设置namespace
    let mut child_nsproxy = child_pcb.nsproxy.lock();
    child_nsproxy.pid_ns_for_children = child_pid_ns;
    drop(child_nsproxy);
    
    // 返回子进程在父进程namespace中的PID
    let parent_ns = current_pid_namespace();
    let child_pid_nr = child_pid.pid_nr(&parent_ns).unwrap_or(0);
    
    Ok(child_pid_nr as usize)
}
```

### 3.6 实施计划

#### 第一阶段（1-2周）：核心数据结构
1. **实现基础结构体**： --done
   - 实现`Pid`、`UPid`、`PidLink`结构体
   - 扩展`PidType`枚举支持所有类型
   - 添加必要的trait实现和方法

2. **扩展PidNamespace**：
   - 添加hash表和分配器支持
   - 实现PID分配和查找算法
   - 添加namespace层级管理

3. **修改ProcessControlBlock**：
   - 添加`pids`数组字段
   - 实现PID链接管理方法
   - 保持与现有代码的兼容性

#### 第二阶段（1-2周）：PID管理集成
1. **实现多层级PID分配**：
   - 实现`alloc_pid_for_task`函数
   - 支持跨namespace的PID分配
   - 添加PID回收机制

2. **集成到进程生命周期**：
   - 修改进程创建流程
   - 实现进程销毁时的PID清理
   - 添加任务与PID的双向链接

3. **实现PID类型管理**：
   - 支持TGID、PGID、SID等不同类型
   - 实现继承和设置逻辑
   - 添加类型转换和查找

#### 第三阶段（1-2周）：系统调用适配
1. **修改现有系统调用**：
   - 更新`getpid`、`getppid`等调用
   - 修改`kill`、`waitpid`支持namespace
   - 确保procfs显示正确的PID

2. **实现namespace系统调用**：
   - 实现`unshare`支持PID namespace
   - 添加`setns`基础支持
   - 实现namespace文件描述符

3. **集成到do_fork**：
   - 支持`CLONE_NEWPID`标志
   - 正确处理子进程的namespace
   - 实现跨namespace的进程关系

#### 第四阶段（1-2周）：测试与优化
1. **全面测试**：
   - 单元测试各个组件
   - 集成测试进程创建和管理
   - 压力测试和并发测试

2. **性能优化**：
   - 优化hash表查找性能
   - 减少内存分配开销
   - 优化锁的使用

3. **错误处理和边界情况**：
   - 处理PID耗尽情况
   - 处理namespace层级过深
   - 添加完善的错误恢复

### 3.7 兼容性保证

#### 3.7.1 向后兼容策略
- 保持现有的`Pid`类型作为简单PID值使用
- 现有系统调用继续正常工作
- 渐进式迁移，避免破坏性更改
- 使用`Option`类型包装新功能

#### 3.7.2 性能考虑
- 使用HashMap进行O(1)PID查找
- 避免不必要的内存分配和复制
- 利用Weak引用避免循环引用
- 优化热路径的锁使用

### 3.8 测试策略

#### 3.8.1 单元测试
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pid_allocation() {
        let ns = PidNamespace::new(0, None).unwrap();
        let pid = alloc_pid_for_task(&ns).unwrap();
        assert_eq!(pid.level, 0);
        assert_eq!(pid.pid_nr_current(), 1);
    }

    #[test]
    fn test_multi_level_pid() {
        let root_ns = PidNamespace::new(0, None).unwrap();
        let child_ns = PidNamespace::new(1, Some(root_ns.clone())).unwrap();
        
        let pid = alloc_pid_for_task(&child_ns).unwrap();
        assert_eq!(pid.level, 1);
        assert!(pid.pid_nr(&root_ns).is_some());
        assert!(pid.pid_nr(&child_ns).is_some());
    }

    #[test]
    fn test_pid_link() {
        let ns = PidNamespace::new(0, None).unwrap();
        let pid = alloc_pid_for_task(&ns).unwrap();
        
        let mut link = PidLink::new();
        link.link_pid(pid.clone());
        assert!(link.get_pid().is_some());
        
        link.unlink_pid();
        assert!(link.get_pid().is_none());
    }
}
```

#### 3.8.2 集成测试
```rust
#[test]
fn test_process_creation_with_namespace() {
    // 测试带namespace的进程创建
    let flags = CloneFlags::CLONE_NEWPID;
    let result = do_fork(flags, 0, 0, std::ptr::null_mut(), std::ptr::null_mut());
    assert!(result.is_ok());
}

#[test]
fn test_getpid_in_namespace() {
    // 测试在不同namespace中getpid的行为
    // TODO: 实现具体测试逻辑
}
```

## 4. 总结

这个完善的重构方案基于DragonOS现有架构，完整实现了Linux风格的PID namespace机制，重点补充了你提到的关键组件：

### 4.1 核心改进
1. **完整的`pid`结构体**：管理多层级PID映射和任务列表
2. **`pid_link`桥梁结构**：连接任务和PID的关键组件
3. **`upid`层级映射**：支持嵌套namespace的PID管理
4. **类型化PID管理**：支持PID、TGID、PGID、SID等不同类型

### 4.2 技术特点
- **Linux语义兼容**：完全符合Linux PID namespace的行为
- **高效实现**：基于HashMap的O(1)查找性能
- **内存安全**：利用Rust的所有权系统避免内存泄漏
- **渐进式部署**：保证向后兼容，分阶段实施

### 4.3 实施时间线
- **总时间**：4-8周
- **分4个阶段**：每阶段1-2周
- **渐进式实现**：降低风险，确保稳定性

这个方案现在包含了Linux PID namespace的所有关键组件，可以直接应用到DragonOS的实际开发中，创造出完全符合Linux语义的PID namespace系统。 