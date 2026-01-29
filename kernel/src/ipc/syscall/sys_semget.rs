use crate::alloc::vec::Vec;
use crate::arch::interrupt::TrapFrame;
use crate::arch::syscall::nr::SYS_SEMGET;
use crate::ipc::semaphore::sysv::{SemFlags, SemKey, IPC_PRIVATE, SEMMSL};
use crate::process::ProcessManager;
use crate::syscall::table::{FormattedSyscallParam, Syscall};
use syscall_table_macros::declare_syscall;
use system_error::SystemError;

pub struct SysSemgetHandle;

pub(super) fn do_kernel_semget(key: SemKey, nsems: usize, semflg: SemFlags) -> Result<usize, SystemError> {
    if nsems > SEMMSL {
        return Err(SystemError::EINVAL);
    }
    let ipcns = ProcessManager::current_ipcns();
    let mut sem_manager = ipcns.sem.lock();
    let cred = ProcessManager::current_pcb().cred();

    match key {
        IPC_PRIVATE => {
            if nsems == 0 {
                return Err(SystemError::EINVAL);
            }
            sem_manager.add(key, nsems, semflg, cred)
        }
        _ => {
            if let Some(id) = sem_manager.get_by_key(key) {
                if semflg.contains(SemFlags::IPC_CREAT | SemFlags::IPC_EXCL) {
                    return Err(SystemError::EEXIST);
                }
                let sem_set = sem_manager.get_by_id(id).ok_or(SystemError::EINVAL)?;
                if nsems > 0 && nsems > sem_set.nsems() {
                    return Err(SystemError::EINVAL);
                }
                Ok(id.data())
            } else {
                if !semflg.contains(SemFlags::IPC_CREAT) {
                    return Err(SystemError::ENOENT);
                }
                if nsems == 0 {
                    return Err(SystemError::EINVAL);
                }
                sem_manager.add(key, nsems, semflg, cred)
            }
        }
    }
}

impl SysSemgetHandle {
    #[inline(always)]
    fn key(args: &[usize]) -> Result<SemKey, SystemError> {
        let key = args[0] as i32;
        if key < 0 {
            return Err(SystemError::EINVAL);
        }
        Ok(SemKey::new(key as usize))
    }

    #[inline(always)]
    fn nsems(args: &[usize]) -> Result<usize, SystemError> {
        let nsems = args[1] as i32;
        if nsems < 0 {
            return Err(SystemError::EINVAL);
        }
        Ok(nsems as usize)
    }

    #[inline(always)]
    fn semflg(args: &[usize]) -> SemFlags {
        SemFlags::from_bits_truncate(args[2] as u32)
    }
}

impl Syscall for SysSemgetHandle {
    fn num_args(&self) -> usize {
        3
    }

    fn handle(&self, args: &[usize], _frame: &mut TrapFrame) -> Result<usize, SystemError> {
        let key = Self::key(args)?;
        let nsems = Self::nsems(args)?;
        let semflg = Self::semflg(args);
        do_kernel_semget(key, nsems, semflg)
    }

    fn entry_format(&self, args: &[usize]) -> Vec<FormattedSyscallParam> {
        let key = args[0] as i32;
        let nsems = args[1] as i32;
        let semflg = args[2] as u32;
        vec![
            FormattedSyscallParam::new("key", format!("{}", key)),
            FormattedSyscallParam::new("nsems", format!("{}", nsems)),
            FormattedSyscallParam::new("semflg", format!("{:#x}", semflg)),
        ]
    }
}

declare_syscall!(SYS_SEMGET, SysSemgetHandle);
