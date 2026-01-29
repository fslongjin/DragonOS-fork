use alloc::sync::{Arc, Weak};
use hashbrown::HashMap;
use system_error::SystemError;

use crate::process::ProcessControlBlock;

use super::sem::SemBuf;
use super::sem_set::{SemId, SemaphoreSet};

#[derive(Debug, Clone, Copy)]
pub struct SemUndoEntry {
    pub sem_num: u16,
    pub adj: i32,
}

pub type SemUndoList = HashMap<SemId, Weak<SemaphoreSet>>;

pub fn record_sem_undo_for_with_set(
    pcb: &Arc<ProcessControlBlock>,
    sem_id: SemId,
    sem_set: &Arc<SemaphoreSet>,
    sops: &[SemBuf],
) -> Result<(), SystemError> {
    sem_set.record_undo(pcb.raw_pid(), sops)?;
    let mut undo = pcb.sem_undo.lock();
    undo.insert(sem_id, Arc::downgrade(sem_set));
    Ok(())
}

pub fn apply_sem_undo_on_exit(pcb: &Arc<ProcessControlBlock>) {
    let undo_sets = {
        let mut undo = pcb.sem_undo.lock();
        if undo.is_empty() {
            return;
        }
        core::mem::take(&mut *undo)
    };
    let pid = pcb.raw_pid();
    for (_sem_id, sem_set) in undo_sets {
        if let Some(sem_set) = sem_set.upgrade() {
            sem_set.apply_undo_for_pid(pid);
        }
    }
}
