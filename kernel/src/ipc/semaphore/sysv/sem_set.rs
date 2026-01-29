use alloc::{collections::VecDeque, sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use alloc::boxed::Box;

use hashbrown::HashMap;
use ida::IdAllocator;
use num::ToPrimitive;
use system_error::SystemError;

use crate::{
    libs::mutex::{Mutex, MutexGuard},
    libs::wait_queue::{TimeoutWaker, Waiter},
    process::{cred::Cred, ProcessManager, RawPid},
    time::{timer::clock, PosixTimeSpec},
    time::timer::Timer,
};

use super::sem::{
    perform_atomic_semop, sops_has_alter, sops_has_undo, PendingOp, SemBuf, SemOpFlags,
    SemUndoContext, Semaphore, Status,
};
use super::undo::SemUndoEntry;

pub const IPC_PRIVATE: SemKey = SemKey::new(0);

int_like!(SemId, usize);
int_like!(SemKey, usize);

bitflags! {
    pub struct SemFlags: u32 {
        const PERM_MASK = 0o777;
        const IPC_CREAT = 0o1000;
        const IPC_EXCL = 0o2000;
    }
}

#[derive(Debug, Clone, Copy)]
pub enum SemCtlCmd {
    IpcRmid = 0,
    IpcSet = 1,
    IpcStat = 2,
    IpcInfo = 3,
    SemStat = 18,
    SemInfo = 19,
    SemStatAny = 20,
    SemGetPid = 11,
    SemGetVal = 12,
    SemGetAll = 13,
    SemGetNcnt = 14,
    SemGetZcnt = 15,
    SemSetVal = 16,
    SemSetAll = 17,
}

impl SemCtlCmd {
    pub fn from_raw(cmd: usize) -> Result<Self, SystemError> {
        let cmd = cmd & 0xff;
        let r = match cmd {
            0 => Self::IpcRmid,
            1 => Self::IpcSet,
            2 => Self::IpcStat,
            3 => Self::IpcInfo,
            11 => Self::SemGetPid,
            12 => Self::SemGetVal,
            13 => Self::SemGetAll,
            14 => Self::SemGetNcnt,
            15 => Self::SemGetZcnt,
            16 => Self::SemSetVal,
            17 => Self::SemSetAll,
            18 => Self::SemStat,
            19 => Self::SemInfo,
            20 => Self::SemStatAny,
            _ => return Err(SystemError::EINVAL),
        };
        Ok(r)
    }
}

bitflags! {
    pub struct PermissionMode: u8 {
        const READ = 0b01;
        const ALTER = 0b10;
    }
}

// linux/include/uapi/linux/sem.h
pub const SEMMNI: usize = 32000;
pub const SEMMSL: usize = 32000;
pub const SEMMNS: usize = SEMMNI * SEMMSL;
pub const SEMOPM: usize = 500;
pub const SEMVMX: i32 = 32767;
pub const SEMAEM: i32 = SEMVMX;
pub const SEMUME: usize = SEMOPM;

#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
pub struct SemInfo {
    pub semmap: i32,
    pub semmni: i32,
    pub semmns: i32,
    pub semmnu: i32,
    pub semmsl: i32,
    pub semopm: i32,
    pub semume: i32,
    pub semusz: i32,
    pub semvmx: i32,
    pub semaem: i32,
}

impl SemInfo {
    pub fn new() -> Self {
        SemInfo {
            semmap: SEMMNS as i32,
            semmni: SEMMNI as i32,
            semmns: SEMMNS as i32,
            semmnu: SEMMNS as i32,
            semmsl: SEMMSL as i32,
            semopm: SEMOPM as i32,
            semume: SEMUME as i32,
            // Linux uapi uses SEMUSZ=20 (sizeof struct sem_undo)
            semusz: 20,
            semvmx: SEMVMX,
            semaem: SEMAEM,
        }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
pub struct IpcPerm {
    pub key: u32,
    pub uid: u32,
    pub gid: u32,
    pub cuid: u32,
    pub cgid: u32,
    pub mode: u16,
    pub _pad1: u16,
    pub seq: u16,
    pub _pad2: u16,
    pub _unused1: u64,
    pub _unused2: u64,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
pub struct SemidDs {
    pub sem_perm: IpcPerm,
    pub sem_otime: u64,
    #[cfg(target_arch = "x86_64")]
    pub _unused1: u64,
    pub sem_ctime: u64,
    #[cfg(target_arch = "x86_64")]
    pub _unused2: u64,
    pub sem_nsems: u64,
    pub _unused3: u64,
    pub _unused4: u64,
}

#[derive(Debug)]
pub struct KernIpcPerm {
    pub id: SemId,
    pub key: SemKey,
    pub uid: usize,
    pub gid: usize,
    pub cuid: usize,
    pub cgid: usize,
    pub mode: u16,
    pub seq: usize,
}

impl KernIpcPerm {
    pub fn new_with_cred(id: SemId, key: SemKey, cred: Arc<Cred>, mode: u16) -> Self {
        KernIpcPerm {
            id,
            key,
            uid: cred.uid.data(),
            gid: cred.gid.data(),
            cuid: cred.uid.data(),
            cgid: cred.gid.data(),
            mode,
            seq: 0,
        }
    }
}

impl TryFrom<&KernIpcPerm> for IpcPerm {
    type Error = SystemError;

    fn try_from(kern: &KernIpcPerm) -> Result<Self, Self::Error> {
        let key = kern
            .key
            .data()
            .to_u32()
            .ok_or(SystemError::EOVERFLOW)?;
        Ok(IpcPerm {
            key,
            uid: kern.uid as u32,
            gid: kern.gid as u32,
            cuid: kern.cuid as u32,
            cgid: kern.cgid as u32,
            mode: kern.mode,
            seq: kern.seq as u16,
            ..IpcPerm::default()
        })
    }
}

#[derive(Debug)]
pub struct SemSetInner {
    pub sems: Box<[Semaphore]>,
    pub pending_alter: VecDeque<Arc<PendingOp>>,
    pub pending_const: VecDeque<Arc<PendingOp>>,
}

#[derive(Debug)]
pub struct SemaphoreSet {
    nsems: usize,
    inner: Mutex<SemSetInner>,
    perm: Mutex<KernIpcPerm>,
    undo: Mutex<HashMap<RawPid, Vec<SemUndoEntry>>>,
    sem_ctime: AtomicU64,
    sem_otime: AtomicU64,
    removed: AtomicBool,
}

impl SemaphoreSet {
    pub fn new(key: SemKey, nsems: usize, mode: u16, cred: Arc<Cred>) -> Result<Self, SystemError> {
        let mut sems = Vec::with_capacity(nsems);
        for _ in 0..nsems {
            sems.push(Semaphore::new(0));
        }
        let id = SemId::new(0);
        let perm = KernIpcPerm::new_with_cred(id, key, cred, mode);
        Ok(Self {
            nsems,
            perm: Mutex::new(perm),
            undo: Mutex::new(HashMap::new()),
            sem_ctime: AtomicU64::new(PosixTimeSpec::now().tv_sec as u64),
            sem_otime: AtomicU64::new(0),
            inner: Mutex::new(SemSetInner {
                sems: sems.into_boxed_slice(),
                pending_alter: VecDeque::new(),
                pending_const: VecDeque::new(),
            }),
            removed: AtomicBool::new(false),
        })
    }

    pub fn inner(&self) -> MutexGuard<'_, SemSetInner> {
        self.inner.lock()
    }

    pub fn nsems(&self) -> usize {
        self.nsems
    }

    pub fn perm(&self) -> MutexGuard<'_, KernIpcPerm> {
        self.perm.lock()
    }

    pub fn update_ctime(&self) {
        self.sem_ctime
            .store(PosixTimeSpec::now().tv_sec as u64, Ordering::Relaxed);
    }

    pub fn update_otime(&self) {
        self.sem_otime
            .store(PosixTimeSpec::now().tv_sec as u64, Ordering::Relaxed);
    }

    pub fn semid_ds(&self) -> Result<SemidDs, SystemError> {
        let perm = self.perm();
        let ipc_perm = IpcPerm::try_from(&*perm)?;
        Ok(SemidDs {
            sem_perm: ipc_perm,
            sem_otime: self.sem_otime.load(Ordering::Relaxed),
            sem_ctime: self.sem_ctime.load(Ordering::Relaxed),
            sem_nsems: self.nsems as u64,
            ..SemidDs::default()
        })
    }

    pub fn setval(&self, sem_num: usize, val: i32, pid: RawPid) -> Result<(), SystemError> {
        if !(0..=SEMVMX).contains(&val) {
            return Err(SystemError::ERANGE);
        }
        let mut inner = self.inner();
        let sem = inner.sems.get_mut(sem_num).ok_or(SystemError::EINVAL)?;
        sem.set_val(val);
        sem.set_latest_modified_pid(pid);
        self.update_ctime();
        self.clear_undo_setval(sem_num as u16);
        let wake_ops = wake_pending_ops(&mut inner);
        drop(inner);
        wake_ops_now(wake_ops);
        Ok(())
    }

    pub fn ipc_set(&self, uid: u32, gid: u32, mode: u16) {
        let mut perm = self.perm.lock();
        perm.uid = uid as usize;
        perm.gid = gid as usize;
        perm.mode = mode & 0o777;
        self.update_ctime();
    }

    pub fn setall(&self, vals: &[u16], pid: RawPid) -> Result<(), SystemError> {
        if vals.len() != self.nsems {
            return Err(SystemError::EINVAL);
        }
        let mut inner = self.inner();
        for (idx, val) in vals.iter().enumerate() {
            let v = *val as i32;
            if !(0..=SEMVMX).contains(&v) {
                return Err(SystemError::ERANGE);
            }
            let sem = inner.sems.get_mut(idx).ok_or(SystemError::EINVAL)?;
            sem.set_val(v);
            sem.set_latest_modified_pid(pid);
        }
        self.update_ctime();
        self.clear_undo_setall();
        let wake_ops = wake_pending_ops(&mut inner);
        drop(inner);
        wake_ops_now(wake_ops);
        Ok(())
    }

    pub fn get_val(&self, sem_num: usize) -> Result<i32, SystemError> {
        let inner = self.inner();
        let sem = inner.sems.get(sem_num).ok_or(SystemError::EINVAL)?;
        Ok(sem.val())
    }

    pub fn get_pid(&self, sem_num: usize) -> Result<RawPid, SystemError> {
        let inner = self.inner();
        let sem = inner.sems.get(sem_num).ok_or(SystemError::EINVAL)?;
        Ok(sem.latest_modified_pid())
    }

    pub fn get_all(&self) -> Vec<u16> {
        let inner = self.inner();
        inner.sems.iter().map(|s| s.val() as u16).collect()
    }

    pub fn pending_const_count(&self, sem_num: u16) -> usize {
        let inner = self.inner();
        inner
            .pending_const
            .iter()
            .filter(|op| {
                op.sops()
                    .first()
                    .map(|sop| sop.sem_num() == sem_num)
                    .unwrap_or(false)
            })
            .count()
    }

    pub fn pending_alter_count(&self, sem_num: u16) -> usize {
        let inner = self.inner();
        inner
            .pending_alter
            .iter()
            .filter(|op| {
                op.sops()
                    .first()
                    .map(|sop| sop.sem_num() == sem_num && sop.sem_op() < 0)
                    .unwrap_or(false)
            })
            .count()
    }

    pub fn is_removed(&self) -> bool {
        self.removed.load(Ordering::Acquire)
    }

    pub fn mark_removed(&self) {
        self.removed.store(true, Ordering::Release);
        let mut inner = self.inner();
        let mut wake_ops = Vec::new();
        for op in inner.pending_alter.drain(..) {
            op.set_status(Status::Removed);
            wake_ops.push(op);
        }
        for op in inner.pending_const.drain(..) {
            op.set_status(Status::Removed);
            wake_ops.push(op);
        }
        drop(inner);
        wake_ops_now(wake_ops);
    }

    pub fn apply_undo_for_pid(&self, pid: RawPid) {
        let mut inner = self.inner();
        let mut undo = self.undo.lock();
        let entries = undo.remove(&pid);
        let Some(entries) = entries else {
            return;
        };
        if entries.is_empty() {
            return;
        }
        for entry in entries {
            if let Some(sem) = inner.sems.get_mut(entry.sem_num as usize) {
                let mut new_val = sem.val().saturating_add(entry.adj);
                if new_val < 0 {
                    new_val = 0;
                } else if new_val > SEMVMX {
                    new_val = SEMVMX;
                }
                sem.set_val(new_val);
                sem.set_latest_modified_pid(pid);
            }
        }
        self.update_otime();
        let wake_ops = wake_pending_ops(&mut inner);
        drop(inner);
        wake_ops_now(wake_ops);
    }

    pub fn semop(self: &Arc<Self>, sops: Vec<SemBuf>, timeout_deadline: Option<u64>) -> Result<(), SystemError> {
        let pid = ProcessManager::current_pid();
        let pcb = ProcessManager::current_pcb();
        let alter = sops_has_alter(&sops);
        let has_undo = sops_has_undo(&sops);
        let (waiter, waker) = Waiter::new_pair();
        let undo_ctx = if has_undo {
            let sem_id = self.perm().id;
            Some(SemUndoContext::new(sem_id, pcb.clone(), Arc::downgrade(self)))
        } else {
            None
        };
        let pending_op = Arc::new(PendingOp::new(
            sops.clone(),
            pid,
            Some(waker.clone()),
            undo_ctx,
        ));

        let mut inner = self.inner();
        if self.is_removed() {
            return Err(SystemError::EIDRM);
        }
        match perform_atomic_semop(&mut inner.sems, &pending_op) {
            Ok(true) => {
                self.update_otime();
                let wake_ops = if alter {
                    wake_pending_ops(&mut inner)
                } else {
                    Vec::new()
                };
                drop(inner);
                wake_ops_now(wake_ops);
                return Ok(());
            }
            Ok(false) => {
                if let Some(deadline) = timeout_deadline {
                    if clock() >= deadline {
                        return Err(SystemError::EAGAIN_OR_EWOULDBLOCK);
                    }
                }
                if alter {
                    inner.pending_alter.push_back(pending_op.clone());
                } else {
                    inner.pending_const.push_back(pending_op.clone());
                }
                drop(inner);

                let timer = if let Some(deadline) = timeout_deadline {
                    let t = Timer::new(TimeoutWaker::new(waker.clone()), deadline);
                    t.activate();
                    Some(t)
                } else {
                    None
                };

                let wait_res = waiter.wait(true);
                let timed_out = timer.as_ref().map(|t| t.timeout()).unwrap_or(false);
                if !timed_out {
                    if let Some(t) = timer {
                        t.cancel();
                    }
                }

                if let Err(e) = wait_res {
                    let mut inner = self.inner();
                    remove_pending(&mut inner, alter, &pending_op);
                    return Err(match e {
                        SystemError::ERESTARTSYS => SystemError::EINTR,
                        _ => e,
                    });
                }

                match pending_op.status() {
                    Status::Normal => {
                        self.update_otime();
                        return Ok(());
                    }
                    Status::Removed => return Err(SystemError::EIDRM),
                    Status::Pending => {
                        let mut inner = self.inner();
                        remove_pending(&mut inner, alter, &pending_op);
                        if timed_out {
                            return Err(SystemError::EAGAIN_OR_EWOULDBLOCK);
                        }
                        return Err(SystemError::EINTR);
                    }
                }
            }
            Err(e) => return Err(e),
        }
    }

    pub fn record_undo(&self, pid: RawPid, sops: &[SemBuf]) -> Result<(), SystemError> {
        let mut deltas: Vec<(u16, i32)> = Vec::new();
        for sop in sops {
            let flags = SemOpFlags::from_bits_truncate(sop.sem_flags() as u16);
            if !flags.contains(SemOpFlags::SEM_UNDO) || sop.sem_op() == 0 {
                continue;
            }
            let adj = -(i32::from(sop.sem_op()));
            if let Some(entry) = deltas.iter_mut().find(|e| e.0 == sop.sem_num()) {
                entry.1 = entry.1.checked_add(adj).ok_or(SystemError::ERANGE)?;
            } else {
                deltas.push((sop.sem_num(), adj));
            }
        }
        if deltas.is_empty() {
            return Ok(());
        }
        let mut undo = self.undo.lock();
        let entries = undo.entry(pid).or_insert_with(Vec::new);
        let mut new_entries = 0usize;
        for (sem_num, delta) in &deltas {
            let current = entries
                .iter()
                .find(|e| e.sem_num == *sem_num)
                .map(|e| e.adj)
                .unwrap_or(0);
            let next = current.checked_add(*delta).ok_or(SystemError::ERANGE)?;
            if next < (-SEMAEM - 1) || next > SEMAEM {
                return Err(SystemError::ERANGE);
            }
            if entries.iter().all(|e| e.sem_num != *sem_num) {
                new_entries += 1;
            }
        }
        if entries.len() + new_entries > SEMUME {
            return Err(SystemError::ENOSPC);
        }
        for (sem_num, delta) in deltas {
            if let Some(entry) = entries.iter_mut().find(|e| e.sem_num == sem_num) {
                entry.adj = entry.adj.checked_add(delta).ok_or(SystemError::ERANGE)?;
            } else {
                entries.push(SemUndoEntry {
                    sem_num,
                    adj: delta,
                });
            }
        }
        Ok(())
    }

    fn clear_undo_setval(&self, sem_num: u16) {
        let mut undo = self.undo.lock();
        for entries in undo.values_mut() {
            if let Some(entry) = entries.iter_mut().find(|e| e.sem_num == sem_num) {
                entry.adj = 0;
            }
        }
    }

    fn clear_undo_setall(&self) {
        let mut undo = self.undo.lock();
        for entries in undo.values_mut() {
            for entry in entries.iter_mut() {
                entry.adj = 0;
            }
        }
    }
}

impl Drop for SemaphoreSet {
    fn drop(&mut self) {
        self.removed.store(true, Ordering::Release);
    }
}

fn wake_pending_ops(inner: &mut SemSetInner) -> Vec<Arc<PendingOp>> {
    let mut wake_queue: Vec<Arc<PendingOp>> = Vec::new();
    let mut progress = true;
    while progress {
        progress = false;

        let mut idx = 0;
        while idx < inner.pending_const.len() {
            let op = inner.pending_const.get(idx).cloned();
            let Some(op) = op else { break };
            match perform_atomic_semop(&mut inner.sems, &op) {
                Ok(true) => {
                    inner.pending_const.remove(idx);
                    wake_queue.push(op);
                    progress = true;
                }
                Ok(false) => idx += 1,
                Err(_) => idx += 1,
            }
        }

        let mut idx = 0;
        while idx < inner.pending_alter.len() {
            let op = inner.pending_alter.get(idx).cloned();
            let Some(op) = op else { break };
            match perform_atomic_semop(&mut inner.sems, &op) {
                Ok(true) => {
                    inner.pending_alter.remove(idx);
                    wake_queue.push(op);
                    progress = true;
                }
                Ok(false) => idx += 1,
                Err(_) => idx += 1,
            }
        }
    }

    for op in &wake_queue {
        op.set_status(Status::Normal);
    }
    wake_queue
}

fn remove_pending(inner: &mut SemSetInner, alter: bool, target: &Arc<PendingOp>) {
    let list = if alter {
        &mut inner.pending_alter
    } else {
        &mut inner.pending_const
    };
    if let Some(pos) = list.iter().position(|op| Arc::ptr_eq(op, target)) {
        list.remove(pos);
    }
}

fn wake_ops_now(ops: Vec<Arc<PendingOp>>) {
    for op in ops {
        if let Some(waker) = op.waker() {
            waker.wake();
        }
    }
}

#[derive(Debug)]
pub struct SemManager {
    id_allocator: IdAllocator,
    id2set: HashMap<SemId, Arc<SemaphoreSet>>,
    key2id: HashMap<SemKey, SemId>,
}

impl Default for SemManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SemManager {
    pub fn new() -> Self {
        Self {
            id_allocator: IdAllocator::new(1, usize::MAX - 1).unwrap(),
            id2set: HashMap::new(),
            key2id: HashMap::new(),
        }
    }

    pub fn add(
        &mut self,
        key: SemKey,
        nsems: usize,
        semflg: SemFlags,
        cred: Arc<Cred>,
    ) -> Result<usize, SystemError> {
        if !(1..=SEMMSL).contains(&nsems) {
            return Err(SystemError::EINVAL);
        }
        if self.id2set.len() >= SEMMNI {
            return Err(SystemError::ENOSPC);
        }
        let id = self.id_allocator.alloc().ok_or(SystemError::ENOSPC)?;
        let sem_id = SemId::new(id);
        let sem_set = SemaphoreSet::new(key, nsems, (semflg.bits() & 0o777) as u16, cred)?;
        {
            let mut perm = sem_set.perm.lock();
            perm.id = sem_id;
        }
        let sem_set = Arc::new(sem_set);
        if key != IPC_PRIVATE {
            self.key2id.insert(key, sem_id);
        }
        self.id2set.insert(sem_id, sem_set);
        Ok(sem_id.data())
    }

    pub fn get_by_id(&self, id: SemId) -> Option<Arc<SemaphoreSet>> {
        self.id2set.get(&id).cloned()
    }

    pub fn max_id(&self) -> usize {
        self.id2set
            .keys()
            .map(|id| id.data())
            .max()
            .unwrap_or(0)
    }

    pub fn get_by_key(&self, key: SemKey) -> Option<SemId> {
        self.key2id.get(&key).copied()
    }

    pub fn remove(&mut self, id: SemId) -> Result<(), SystemError> {
        let sem_set = self.id2set.remove(&id).ok_or(SystemError::EINVAL)?;
        let key = sem_set.perm().key;
        if key != IPC_PRIVATE {
            self.key2id.remove(&key);
        }
        self.id_allocator.free(id.0);
        sem_set.mark_removed();
        Ok(())
    }
}

pub fn check_perm(perm: &KernIpcPerm, required: PermissionMode, cred: &Cred) -> Result<(), SystemError> {
    if required.is_empty() {
        return Ok(());
    }
    let euid = cred.euid.data();
    let egid = cred.egid.data();
    let in_group = |gid: usize| -> bool {
        if egid == gid {
            return true;
        }
        if let Some(group_info) = &cred.group_info {
            return group_info.gids.iter().any(|g| g.data() == gid);
        }
        cred.groups.iter().any(|g| g.data() == gid)
    };
    let mode = perm.mode;
    let is_owner = euid == perm.uid || euid == perm.cuid;
    let is_group = in_group(perm.gid) || in_group(perm.cgid);

    let read_bit = if is_owner { 0o400 } else if is_group { 0o040 } else { 0o004 };
    let write_bit = if is_owner { 0o200 } else if is_group { 0o020 } else { 0o002 };

    if required.contains(PermissionMode::READ) && (mode & read_bit) == 0 {
        return Err(SystemError::EACCES);
    }
    if required.contains(PermissionMode::ALTER) && (mode & write_bit) == 0 {
        return Err(SystemError::EACCES);
    }
    Ok(())
}
