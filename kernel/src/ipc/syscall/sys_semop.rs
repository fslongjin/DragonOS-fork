use alloc::vec::Vec;
use core::mem::size_of;

use crate::arch::interrupt::TrapFrame;
use crate::arch::syscall::nr::SYS_SEMOP;
use crate::ipc::semaphore::sysv::{sem_set::check_perm, sem_set::PermissionMode, SemBuf, SemId, SEMOPM};
use crate::ipc::signal::{RestartBlock, RestartBlockData, RestartFn};
use crate::process::ProcessManager;
use crate::syscall::table::{FormattedSyscallParam, Syscall};
use crate::syscall::user_access::UserBufferReader;
use crate::time::{timer::clock, Duration};
use crate::time::timer::next_n_us_timer_jiffies;
use crate::time::jiffies::NSEC_PER_JIFFY;
use crate::mm::VirtAddr;
use syscall_table_macros::declare_syscall;
use system_error::SystemError;

pub struct SysSemopHandle;

#[derive(Debug)]
pub struct RestartFnSemtimedop;

impl RestartFn for RestartFnSemtimedop {
    fn call(&self, data: &mut RestartBlockData) -> Result<usize, SystemError> {
        if let RestartBlockData::Semtimedop {
            semid,
            tsops_ptr,
            nsops,
            deadline_jiffies,
        } = data
        {
            let now = clock();
            if now >= *deadline_jiffies {
                return Err(SystemError::EAGAIN_OR_EWOULDBLOCK);
            }
            let remaining_jiffies = *deadline_jiffies - now;
            let remaining_ns = remaining_jiffies.saturating_mul(NSEC_PER_JIFFY as u64);
            let remaining_us = (remaining_ns + 999) / 1000;
            let timeout = Some(Duration::from_micros(remaining_us));
            return do_kernel_semtimedop(
                SemId::new(*semid),
                tsops_ptr.data() as *const SemBuf,
                *nsops,
                timeout,
                true,
            );
        }
        panic!("RestartFnSemtimedop called with wrong data type: {:?}", data);
    }
}

pub(super) fn do_kernel_semtimedop(
    semid: SemId,
    tsops: *const SemBuf,
    nsops: usize,
    timeout: Option<Duration>,
    from_user: bool,
) -> Result<usize, SystemError> {
    if nsops == 0 {
        return Err(SystemError::EINVAL);
    }
    if nsops > SEMOPM {
        return Err(SystemError::E2BIG);
    }

    let reader = UserBufferReader::new(tsops, nsops * size_of::<SemBuf>(), from_user)?;
    let mut sops = Vec::with_capacity(nsops);
    for i in 0..nsops {
        let mut sop = SemBuf::default();
        reader.copy_one_from_user(&mut sop, i * size_of::<SemBuf>())?;
        sops.push(sop);
    }

    let ipcns = ProcessManager::current_ipcns();
    let sem_set = {
        let manager = ipcns.sem.lock();
        manager.get_by_id(semid).ok_or(SystemError::EINVAL)?
    };
    let has_alter = sops.iter().any(|sop| sop.sem_op != 0);
    let has_read = sops.iter().any(|sop| sop.sem_op == 0);
    let mut required = PermissionMode::empty();
    if has_alter {
        required.insert(PermissionMode::ALTER);
    }
    if has_read {
        required.insert(PermissionMode::READ);
    }
    let cred = ProcessManager::current_pcb().cred();
    {
        let perm = sem_set.perm();
        check_perm(&perm, required, &cred)?;
    }

    let deadline_jiffies = timeout.map(|t| next_n_us_timer_jiffies(t.total_micros()));
    let res = sem_set.semop(sops, deadline_jiffies);
    match res {
        Ok(()) => Ok(0),
        Err(SystemError::ERESTARTSYS) if deadline_jiffies.is_some() && from_user => {
            let data = RestartBlockData::Semtimedop {
                semid: semid.data(),
                tsops_ptr: VirtAddr::new(tsops as usize),
                nsops,
                deadline_jiffies: deadline_jiffies.unwrap(),
            };
            let rb = RestartBlock::new(&RestartFnSemtimedop, data);
            ProcessManager::current_pcb().set_restart_fn(Some(rb))
        }
        Err(e) => Err(e),
    }
}

impl SysSemopHandle {
    #[inline(always)]
    fn semid(args: &[usize]) -> Result<SemId, SystemError> {
        let semid = args[0] as i32;
        if semid <= 0 {
            return Err(SystemError::EINVAL);
        }
        Ok(SemId::new(semid as usize))
    }

    #[inline(always)]
    fn tsops(args: &[usize]) -> *const SemBuf {
        args[1] as *const SemBuf
    }

    #[inline(always)]
    fn nsops(args: &[usize]) -> usize {
        args[2]
    }
}

impl Syscall for SysSemopHandle {
    fn num_args(&self) -> usize {
        3
    }

    fn handle(&self, args: &[usize], frame: &mut TrapFrame) -> Result<usize, SystemError> {
        let semid = Self::semid(args)?;
        let tsops = Self::tsops(args);
        let nsops = Self::nsops(args);
        do_kernel_semtimedop(semid, tsops, nsops, None, frame.is_from_user())
    }

    fn entry_format(&self, args: &[usize]) -> Vec<FormattedSyscallParam> {
        vec![
            FormattedSyscallParam::new("semid", format!("{}", args[0] as i32)),
            FormattedSyscallParam::new("sops", format!("{:#x}", args[1])),
            FormattedSyscallParam::new("nsops", format!("{}", args[2])),
        ]
    }
}

declare_syscall!(SYS_SEMOP, SysSemopHandle);
