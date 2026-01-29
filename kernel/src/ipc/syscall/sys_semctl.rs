use alloc::vec::Vec;
use core::mem::size_of;

use crate::arch::interrupt::TrapFrame;
use crate::arch::syscall::nr::SYS_SEMCTL;
use crate::ipc::semaphore::sysv::{
    sem_set::{check_perm, PermissionMode},
    SemCtlCmd, SemInfo, SemId, SemidDs,
};
use crate::process::ProcessManager;
use crate::syscall::table::{FormattedSyscallParam, Syscall};
use crate::syscall::user_access::{UserBufferReader, UserBufferWriter};
use syscall_table_macros::declare_syscall;
use system_error::SystemError;

pub struct SysSemctlHandle;

impl SysSemctlHandle {
    #[inline(always)]
    fn semid(args: &[usize]) -> Result<SemId, SystemError> {
        let semid = args[0] as i32;
        if semid <= 0 {
            return Err(SystemError::EINVAL);
        }
        Ok(SemId::new(semid as usize))
    }

    #[inline(always)]
    fn semnum(args: &[usize]) -> usize {
        args[1]
    }

    #[inline(always)]
    fn cmd(args: &[usize]) -> Result<SemCtlCmd, SystemError> {
        SemCtlCmd::from_raw(args[2])
    }

    #[inline(always)]
    fn arg(args: &[usize]) -> usize {
        args[3]
    }
}

impl Syscall for SysSemctlHandle {
    fn num_args(&self) -> usize {
        4
    }

    fn handle(&self, args: &[usize], frame: &mut TrapFrame) -> Result<usize, SystemError> {
        let semnum = Self::semnum(args);
        let cmd = Self::cmd(args)?;
        let arg = Self::arg(args);

        let ipcns = ProcessManager::current_ipcns();
        let cred = ProcessManager::current_pcb().cred();

        match cmd {
            SemCtlCmd::IpcInfo | SemCtlCmd::SemInfo => {
                let info = SemInfo::new();
                let max_id = {
                    let manager = ipcns.sem.lock();
                    manager.max_id()
                };
                let mut writer = UserBufferWriter::new(
                    arg as *mut u8,
                    core::mem::size_of::<SemInfo>(),
                    frame.is_from_user(),
                )?;
                writer.copy_one_to_user(&info, 0)?;
                return Ok(max_id);
            }
            SemCtlCmd::IpcRmid => {
                let semid = Self::semid(args)?;
                let sem_set = {
                    let manager = ipcns.sem.lock();
                    manager.get_by_id(semid).ok_or(SystemError::EINVAL)?
                };
                {
                    let perm = sem_set.perm();
                    let euid = cred.uid.data();
                    let can_remove = euid == perm.uid || euid == perm.cuid;
                    if !can_remove {
                        return Err(SystemError::EPERM);
                    }
                }
                let mut manager = ipcns.sem.lock();
                manager.remove(semid)?;
                Ok(0)
            }
            SemCtlCmd::IpcStat => {
                let semid = Self::semid(args)?;
                let sem_set = {
                    let manager = ipcns.sem.lock();
                    manager.get_by_id(semid).ok_or(SystemError::EINVAL)?
                };
                {
                    let perm = sem_set.perm();
                    check_perm(&perm, PermissionMode::READ, &cred)?;
                }
                let semid_ds = sem_set.semid_ds()?;
                let mut writer = UserBufferWriter::new(arg as *mut u8, size_of::<SemidDs>(), frame.is_from_user())?;
                writer.copy_one_to_user(&semid_ds, 0)?;
                Ok(0)
            }
            SemCtlCmd::IpcSet => {
                let semid = Self::semid(args)?;
                let sem_set = {
                    let manager = ipcns.sem.lock();
                    manager.get_by_id(semid).ok_or(SystemError::EINVAL)?
                };
                {
                    let perm = sem_set.perm();
                    let euid = cred.euid.data();
                    let can_set = euid == perm.uid || euid == perm.cuid;
                    if !can_set {
                        return Err(SystemError::EPERM);
                    }
                }
                let reader =
                    UserBufferReader::new(arg as *const u8, size_of::<SemidDs>(), frame.is_from_user())?;
                let mut user_ds = SemidDs::default();
                reader.copy_one_from_user(&mut user_ds, 0)?;
                sem_set.ipc_set(user_ds.sem_perm.uid, user_ds.sem_perm.gid, user_ds.sem_perm.mode);
                Ok(0)
            }
            SemCtlCmd::SemGetVal => {
                let semid = Self::semid(args)?;
                let sem_set = {
                    let manager = ipcns.sem.lock();
                    manager.get_by_id(semid).ok_or(SystemError::EINVAL)?
                };
                let perm = sem_set.perm();
                check_perm(&perm, PermissionMode::READ, &cred)?;
                let val = sem_set.get_val(semnum)?;
                Ok(val as usize)
            }
            SemCtlCmd::SemGetPid => {
                let semid = Self::semid(args)?;
                let sem_set = {
                    let manager = ipcns.sem.lock();
                    manager.get_by_id(semid).ok_or(SystemError::EINVAL)?
                };
                let perm = sem_set.perm();
                check_perm(&perm, PermissionMode::READ, &cred)?;
                let pid = sem_set.get_pid(semnum)?;
                Ok(pid.data())
            }
            SemCtlCmd::SemGetZcnt => {
                let semid = Self::semid(args)?;
                let sem_set = {
                    let manager = ipcns.sem.lock();
                    manager.get_by_id(semid).ok_or(SystemError::EINVAL)?
                };
                let perm = sem_set.perm();
                check_perm(&perm, PermissionMode::READ, &cred)?;
                Ok(sem_set.pending_const_count(semnum as u16))
            }
            SemCtlCmd::SemGetNcnt => {
                let semid = Self::semid(args)?;
                let sem_set = {
                    let manager = ipcns.sem.lock();
                    manager.get_by_id(semid).ok_or(SystemError::EINVAL)?
                };
                let perm = sem_set.perm();
                check_perm(&perm, PermissionMode::READ, &cred)?;
                Ok(sem_set.pending_alter_count(semnum as u16))
            }
            SemCtlCmd::SemSetVal => {
                let semid = Self::semid(args)?;
                let sem_set = {
                    let manager = ipcns.sem.lock();
                    manager.get_by_id(semid).ok_or(SystemError::EINVAL)?
                };
                let perm = sem_set.perm();
                check_perm(&perm, PermissionMode::ALTER, &cred)?;
                let val = arg as i32;
                if val < 0 {
                    return Err(SystemError::ERANGE);
                }
                sem_set.setval(semnum, val, ProcessManager::current_pid())?;
                Ok(0)
            }
            SemCtlCmd::SemGetAll => {
                let semid = Self::semid(args)?;
                let sem_set = {
                    let manager = ipcns.sem.lock();
                    manager.get_by_id(semid).ok_or(SystemError::EINVAL)?
                };
                let perm = sem_set.perm();
                check_perm(&perm, PermissionMode::READ, &cred)?;
                let vals = sem_set.get_all();
                let mut writer = UserBufferWriter::new(
                    arg as *mut u8,
                    vals.len() * core::mem::size_of::<u16>(),
                    frame.is_from_user(),
                )?;
                writer.copy_to_user(&vals, 0)?;
                Ok(0)
            }
            SemCtlCmd::SemSetAll => {
                let semid = Self::semid(args)?;
                let sem_set = {
                    let manager = ipcns.sem.lock();
                    manager.get_by_id(semid).ok_or(SystemError::EINVAL)?
                };
                let perm = sem_set.perm();
                check_perm(&perm, PermissionMode::ALTER, &cred)?;
                let nsems = sem_set.nsems();
                let reader = UserBufferReader::new(
                    arg as *const u8,
                    nsems * core::mem::size_of::<u16>(),
                    frame.is_from_user(),
                )?;
                let mut vals = Vec::with_capacity(nsems);
                for i in 0..nsems {
                    let mut v: u16 = 0;
                    reader.copy_one_from_user(&mut v, i * core::mem::size_of::<u16>())?;
                    vals.push(v);
                }
                sem_set.setall(&vals, ProcessManager::current_pid())?;
                Ok(0)
            }
            SemCtlCmd::SemStat | SemCtlCmd::SemStatAny => {
                let semid = Self::semid(args)?;
                let sem_set = {
                    let manager = ipcns.sem.lock();
                    manager.get_by_id(semid).ok_or(SystemError::EINVAL)?
                };
                if matches!(cmd, SemCtlCmd::SemStat) {
                    let perm = sem_set.perm();
                    check_perm(&perm, PermissionMode::READ, &cred)?;
                }
                let semid_ds = sem_set.semid_ds()?;
                let mut writer = UserBufferWriter::new(arg as *mut u8, size_of::<SemidDs>(), frame.is_from_user())?;
                writer.copy_one_to_user(&semid_ds, 0)?;
                Ok(semid.data())
            }
        }
    }

    fn entry_format(&self, args: &[usize]) -> Vec<FormattedSyscallParam> {
        vec![
            FormattedSyscallParam::new("semid", format!("{}", args[0] as i32)),
            FormattedSyscallParam::new("semnum", format!("{}", args[1])),
            FormattedSyscallParam::new("cmd", format!("{:#x}", args[2])),
            FormattedSyscallParam::new("arg", format!("{:#x}", args[3])),
        ]
    }
}

declare_syscall!(SYS_SEMCTL, SysSemctlHandle);
