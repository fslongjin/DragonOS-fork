use alloc::{sync::{Arc, Weak}, vec::Vec};
use core::{
    slice::Iter,
    sync::atomic::{AtomicU8, Ordering},
};

use crate::{
    libs::wait_queue::Waker,
    process::{ProcessControlBlock, RawPid},
};
use system_error::SystemError;

use super::sem_set::{SemId, SemaphoreSet, SEMVMX};

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct SemBuf {
    pub sem_num: u16,
    pub sem_op: i16,
    pub sem_flags: i16,
}

impl SemBuf {
    pub fn sem_num(&self) -> u16 {
        self.sem_num
    }

    pub fn sem_op(&self) -> i16 {
        self.sem_op
    }

    pub fn sem_flags(&self) -> i16 {
        self.sem_flags
    }
}

bitflags! {
    pub struct SemOpFlags: u16 {
        const SEM_UNDO = 0x1000;
        const IPC_NOWAIT = 0o4000;
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pending = 0,
    Normal = 1,
    Removed = 2,
}

#[derive(Debug)]
pub struct PendingOp {
    sops: Vec<SemBuf>,
    status: AtomicU8,
    waker: Option<Arc<Waker>>,
    pid: RawPid,
    undo_ctx: Option<SemUndoContext>,
}

#[derive(Debug, Clone)]
pub struct SemUndoContext {
    sem_id: SemId,
    pcb: Arc<ProcessControlBlock>,
    sem_set: Weak<SemaphoreSet>,
}

impl SemUndoContext {
    pub fn new(sem_id: SemId, pcb: Arc<ProcessControlBlock>, sem_set: Weak<SemaphoreSet>) -> Self {
        Self {
            sem_id,
            pcb,
            sem_set,
        }
    }
}

impl PendingOp {
    pub fn new(
        sops: Vec<SemBuf>,
        pid: RawPid,
        waker: Option<Arc<Waker>>,
        undo_ctx: Option<SemUndoContext>,
    ) -> Self {
        Self {
            sops,
            status: AtomicU8::new(Status::Pending as u8),
            waker,
            pid,
            undo_ctx,
        }
    }

    pub fn sops_iter(&self) -> Iter<'_, SemBuf> {
        self.sops.iter()
    }

    pub fn sops(&self) -> &[SemBuf] {
        &self.sops
    }

    pub fn set_status(&self, status: Status) {
        self.status.store(status as u8, Ordering::Release);
    }

    pub fn status(&self) -> Status {
        match self.status.load(Ordering::Acquire) {
            1 => Status::Normal,
            2 => Status::Removed,
            _ => Status::Pending,
        }
    }

    pub fn waker(&self) -> Option<Arc<Waker>> {
        self.waker.clone()
    }

    pub fn pid(&self) -> RawPid {
        self.pid
    }

    pub fn undo_ctx(&self) -> Option<&SemUndoContext> {
        self.undo_ctx.as_ref()
    }
}

#[derive(Debug)]
pub struct Semaphore {
    val: i32,
    latest_modified_pid: crate::process::RawPid,
}

impl Semaphore {
    pub fn new(val: i32) -> Self {
        Self {
            val,
            latest_modified_pid: crate::process::RawPid::new(0),
        }
    }

    pub fn val(&self) -> i32 {
        self.val
    }

    pub fn set_val(&mut self, val: i32) {
        self.val = val;
    }

    pub fn latest_modified_pid(&self) -> crate::process::RawPid {
        self.latest_modified_pid
    }

    pub fn set_latest_modified_pid(&mut self, pid: crate::process::RawPid) {
        self.latest_modified_pid = pid;
    }
}

pub fn perform_atomic_semop(
    sems: &mut [Semaphore],
    pending_op: &PendingOp,
) -> Result<bool, SystemError> {
    let mut tmp_vals: Vec<i32> = sems.iter().map(|s| s.val()).collect();
    for op in pending_op.sops_iter() {
        let idx = op.sem_num as usize;
        let cur = *tmp_vals.get(idx).ok_or(SystemError::EFBIG)?;
        let flags = SemOpFlags::from_bits_truncate(op.sem_flags as u16);

        if op.sem_op == 0 && cur != 0 {
            if flags.contains(SemOpFlags::IPC_NOWAIT) {
                return Err(SystemError::EAGAIN_OR_EWOULDBLOCK);
            }
            return Ok(false);
        }

        let result = cur + i32::from(op.sem_op);
        if result < 0 {
            if flags.contains(SemOpFlags::IPC_NOWAIT) {
                return Err(SystemError::EAGAIN_OR_EWOULDBLOCK);
            }
            return Ok(false);
        }
        if result > SEMVMX {
            return Err(SystemError::ERANGE);
        }
        tmp_vals[idx] = result;
    }

    if let Some(undo_ctx) = pending_op.undo_ctx() {
        let sem_set = undo_ctx.sem_set.upgrade().ok_or(SystemError::EIDRM)?;
        super::undo::record_sem_undo_for_with_set(
            &undo_ctx.pcb,
            undo_ctx.sem_id,
            &sem_set,
            pending_op.sops(),
        )?;
    }

    for (idx, new_val) in tmp_vals.into_iter().enumerate() {
        if sems[idx].val() != new_val {
            sems[idx].set_val(new_val);
            sems[idx].set_latest_modified_pid(pending_op.pid());
        }
    }

    Ok(true)
}

pub fn sops_has_alter(sops: &[SemBuf]) -> bool {
    sops.iter().any(|sop| sop.sem_op != 0)
}

pub fn sops_has_undo(sops: &[SemBuf]) -> bool {
    sops.iter().any(|sop| {
        let flags = SemOpFlags::from_bits_truncate(sop.sem_flags as u16);
        sop.sem_op != 0 && flags.contains(SemOpFlags::SEM_UNDO)
    })
}
