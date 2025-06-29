use core::fmt::Debug;
use core::sync::atomic::{AtomicUsize, Ordering};

use alloc::sync::Weak;
use alloc::vec::Vec;

use alloc::sync::Arc;
use hashbrown::HashMap;
use ida::IdAllocator;
use system_error::SystemError;

use crate::libs::spinlock::SpinLock;
use crate::process::fork::CloneFlags;
use crate::process::pid::{Pid, PidType, UPid};
use crate::process::ProcessControlBlock;
use crate::process::RawPid;

use super::nsproxy::NsCommon;

pub struct PidNamespace {
    self_ref: Weak<PidNamespace>,
    /// PID namespace的层级（root = 0）
    pub level: u32,
    /// 父namespace的弱引用
    parent: Option<Weak<PidNamespace>>,

    /// init进程引用
    child_reaper: SpinLock<Option<Weak<ProcessControlBlock>>>,

    inner: SpinLock<InnerPidNamespace>,
}

impl Debug for PidNamespace {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PidNamespace")
            .field("level", &self.level)
            .finish()
    }
}

pub struct InnerPidNamespace {
    pub ns_common: NsCommon,
    /// PID分配器
    ida: IdAllocator,
    /// 已分配的PID数量
    pid_allocated: AtomicUsize,
    /// PID hash表：(pid_nr) -> Arc<Pid>
    pid_hash: HashMap<i32, Arc<Pid>>,
}

impl PidNamespace {
    /// 创建root PID namespace
    pub fn new_root() -> Arc<Self> {
        Arc::new_cyclic(|self_ref| Self {
            self_ref: self_ref.clone(),
            level: 0,
            parent: None,
            child_reaper: SpinLock::new(None),
            inner: SpinLock::new(InnerPidNamespace {
                ns_common: NsCommon {
                    stashed: core::sync::atomic::AtomicIsize::new(0),
                },
                ida: IdAllocator::new(1, i32::MAX as usize).unwrap(),
                pid_allocated: AtomicUsize::new(0),
                pid_hash: HashMap::new(),
            }),
        })
    }

    /// 创建新的PID namespace
    pub fn new_child(parent: Arc<PidNamespace>) -> Result<Arc<Self>, SystemError> {
        let level = parent.level + 1;
        
        // 检查层级深度限制
        if level > 32 {
            return Err(SystemError::EUSERS);
        }

        Ok(Arc::new_cyclic(|self_ref| Self {
            self_ref: self_ref.clone(),
            level,
            parent: Some(Arc::downgrade(&parent)),
            child_reaper: SpinLock::new(None),
            inner: SpinLock::new(InnerPidNamespace {
                ns_common: NsCommon {
                    stashed: core::sync::atomic::AtomicIsize::new(0),
                },
                ida: IdAllocator::new(1, i32::MAX as usize).unwrap(),
                pid_allocated: AtomicUsize::new(0),
                pid_hash: HashMap::new(),
            }),
        }))
    }

    /// https://code.dragonos.org.cn/xref/linux-6.6.21/kernel/pid_namespace.c#145
    pub(super) fn copy_pid_ns(&self, clone_flags: &CloneFlags) -> Result<Arc<Self>, SystemError> {
        if !clone_flags.contains(CloneFlags::CLONE_NEWPID) {
            return Ok(self.self_ref.upgrade().unwrap());
        }

        // 创建新的PID namespace
        Self::new_child(self.self_ref.upgrade().unwrap())
    }

    /// 分配新的PID
    pub fn alloc_pid(&self) -> Option<i32> {
        let mut inner = self.inner.lock();
        let pid_nr = inner.ida.alloc().map(|id| id as i32)?;
        inner.pid_allocated.fetch_add(1, Ordering::Relaxed);
        Some(pid_nr)
    }

    /// 释放PID
    pub fn free_pid(&self, pid: i32) {
        let mut inner = self.inner.lock();
        inner.ida.free(pid as usize);
        inner.pid_allocated.fetch_sub(1, Ordering::Relaxed);
        
        // 从hash表中移除
        inner.pid_hash.remove(&pid);
    }

    /// 在hash表中注册PID
    pub fn register_pid(&self, pid_nr: i32, pid: Arc<Pid>) {
        let mut inner = self.inner.lock();
        inner.pid_hash.insert(pid_nr, pid);
    }

    /// 根据PID号查找Pid结构体
    pub fn find_pid(&self, pid_nr: i32) -> Option<Arc<Pid>> {
        let inner = self.inner.lock();
        inner.pid_hash.get(&pid_nr).cloned()
    }

    /// 获取已分配的PID数量
    pub fn pid_allocated(&self) -> usize {
        let inner = self.inner.lock();
        inner.pid_allocated.load(Ordering::Relaxed)
    }

    /// 获取父namespace
    pub fn parent(&self) -> Option<Arc<PidNamespace>> {
        self.parent.as_ref().and_then(|p| p.upgrade())
    }

    /// 设置init进程（PID 1）
    pub fn set_child_reaper(&self, reaper: Weak<ProcessControlBlock>) {
        let mut child_reaper = self.child_reaper.lock();
        *child_reaper = Some(reaper);
    }

    /// 获取init进程（PID 1）
    pub fn child_reaper(&self) -> Option<Arc<ProcessControlBlock>> {
        let child_reaper = self.child_reaper.lock();
        child_reaper.as_ref().and_then(|r| r.upgrade())
    }

    /// 检查PID是否已分配
    pub fn pid_exists(&self, pid_nr: i32) -> bool {
        let inner = self.inner.lock();
        inner.ida.exists(pid_nr as usize)
    }

    /// 获取namespace层级
    pub fn level(&self) -> u32 {
        self.level
    }

    /// 检查是否为根namespace
    pub fn is_root(&self) -> bool {
        self.level == 0
    }

    /// 获取最大PID值
    pub fn max_pid(&self) -> usize {
        let inner = self.inner.lock();
        inner.ida.get_max_id()
    }

    /// 获取可用PID数量
    pub fn available_pids(&self) -> usize {
        let inner = self.inner.lock();
        inner.ida.available()
    }
}

/// 为新进程分配PID
pub fn alloc_pid_for_task(ns: &Arc<PidNamespace>) -> Result<Arc<Pid>, SystemError> {
    let level = ns.level as usize;
    let pid = Pid::new(level);
    
    // 从当前namespace开始，向上遍历所有父namespace
    let mut current_ns = Some(ns.clone());
    let mut upids = Vec::with_capacity(level + 1);
    
    while let Some(namespace) = current_ns {
        // 在当前namespace中分配PID
        let pid_nr = namespace.alloc_pid()
            .ok_or(SystemError::ENOMEM)?;
        
        // 创建UPid
        let upid = UPid::new(pid_nr, namespace.clone());
        upids.push(upid);
        
        // 在hash表中注册
        namespace.register_pid(pid_nr, pid.clone());
        
        // 移动到父namespace
        current_ns = namespace.parent();
    }
    
    // 设置numbers数组（从根namespace到当前namespace的顺序）
    upids.reverse();
    for upid in upids {
        pid.add_upid(upid);
    }
    
    Ok(pid)
}

/// 在指定namespace中查找PID
pub fn find_pid_ns(pid_nr: i32, ns: &Arc<PidNamespace>) -> Option<Arc<Pid>> {
    ns.find_pid(pid_nr)
}

/// 根据PID查找对应的任务
pub fn find_task_by_pid_ns(
    pid_nr: i32, 
    pid_type: PidType, 
    ns: &Arc<PidNamespace>
) -> Option<Arc<ProcessControlBlock>> {
    let pid = find_pid_ns(pid_nr, ns)?;
    pid.get_task(pid_type)
}

/// 释放PID及其在所有namespace中的映射
pub fn free_pid(pid: &Arc<Pid>) {
    pid.for_each_upid(|upid| {
        upid.ns.free_pid(upid.nr);
    });
}
