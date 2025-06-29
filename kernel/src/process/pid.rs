#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum PidType {
    /// pid类型是进程id
    PID = 1,
    TGID = 2,
    PGID = 3,
    SID = 4,
    MAX = 5,
}

impl PidType {
    /// PID类型的最大数量，用于数组大小
    pub const PIDTYPE_MAX: usize = Self::MAX as usize;
}

use crate::libs::spinlock::SpinLock;
use crate::process::namespace::pid_namespace::PidNamespace;
use crate::process::ProcessControlBlock;
use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};

/// Linux风格的PID结构体
/// 管理一个PID在多个namespace层级中的映射
#[derive(Debug)]
pub struct Pid {
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
        // 创建初始化的任务列表数组
        const INIT_LOCK: SpinLock<Vec<Weak<ProcessControlBlock>>> = SpinLock::new(Vec::new());

        Arc::new(Self {
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

    /// 获取PID的层级
    pub fn level(&self) -> usize {
        self.level
    }

    /// 获取所有namespace中的PID映射
    pub fn get_upids(&self) -> Vec<UPid> {
        let numbers = self.numbers.lock();
        numbers.clone()
    }

    /// 遍历所有UPid并执行闭包
    pub fn for_each_upid<F>(&self, mut f: F) 
    where 
        F: FnMut(&UPid),
    {
        let numbers = self.numbers.lock();
        for upid in numbers.iter() {
            f(upid);
        }
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

/// 在特定namespace中的PID信息
#[derive(Debug, Clone)]
pub struct UPid {
    /// 在该namespace中的PID值
    pub nr: i32,
    /// 所属的namespace
    pub ns: Arc<PidNamespace>,
}

impl UPid {
    /// 创建新的UPid
    pub fn new(nr: i32, ns: Arc<PidNamespace>) -> Self {
        Self { nr, ns }
    }
}

/// 连接任务和PID的桥梁结构体
#[derive(Debug)]
pub struct PidLink {
    /// 指向对应的Pid结构体
    pub pid: Option<Arc<Pid>>,
}

impl PidLink {
    /// 创建新的PidLink
    pub fn new() -> Self {
        Self { pid: None }
    }

    /// 链接到指定的PID
    pub fn link_pid(&mut self, pid: Arc<Pid>) {
        self.pid = Some(pid);
    }

    /// 取消PID链接
    pub fn unlink_pid(&mut self) {
        self.pid = None;
    }

    /// 获取链接的PID
    pub fn get_pid(&self) -> Option<&Arc<Pid>> {
        self.pid.as_ref()
    }

    /// 检查是否已链接PID
    pub fn is_linked(&self) -> bool {
        self.pid.is_some()
    }
}

impl Default for PidLink {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for PidLink {
    fn clone(&self) -> Self {
        Self {
            pid: self.pid.clone(),
        }
    }
}
