use core::mem::size_of;
use alloc::vec::Vec;

use crate::arch::interrupt::TrapFrame;
use crate::arch::syscall::nr::SYS_SEMTIMEDOP;
use crate::ipc::semaphore::sysv::SemBuf;
use crate::syscall::table::{FormattedSyscallParam, Syscall};
use crate::syscall::user_access::UserBufferReader;
use crate::time::PosixTimeSpec;
use syscall_table_macros::declare_syscall;
use system_error::SystemError;

use super::sys_semop::do_kernel_semtimedop;

pub struct SysSemtimedopHandle;

impl SysSemtimedopHandle {
    #[inline(always)]
    fn semid(args: &[usize]) -> Result<crate::ipc::semaphore::sysv::SemId, SystemError> {
        let semid = args[0] as i32;
        if semid <= 0 {
            return Err(SystemError::EINVAL);
        }
        Ok(crate::ipc::semaphore::sysv::SemId::new(semid as usize))
    }

    #[inline(always)]
    fn tsops(args: &[usize]) -> *const SemBuf {
        args[1] as *const SemBuf
    }

    #[inline(always)]
    fn nsops(args: &[usize]) -> usize {
        args[2]
    }

    #[inline(always)]
    fn timeout(args: &[usize]) -> *const PosixTimeSpec {
        args[3] as *const PosixTimeSpec
    }
}

impl Syscall for SysSemtimedopHandle {
    fn num_args(&self) -> usize {
        4
    }

    fn handle(&self, args: &[usize], frame: &mut TrapFrame) -> Result<usize, SystemError> {
        let semid = Self::semid(args)?;
        let tsops = Self::tsops(args);
        let nsops = Self::nsops(args);
        let timeout_ptr = Self::timeout(args);

        let timeout = if timeout_ptr.is_null() {
            None
        } else {
            let reader =
                UserBufferReader::new(timeout_ptr, size_of::<PosixTimeSpec>(), frame.is_from_user())?;
            let mut ts = PosixTimeSpec::default();
            reader.copy_one_from_user(&mut ts, 0)?;
            if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 {
                return Err(SystemError::EINVAL);
            }
            Some(crate::time::Duration::from(ts))
        };

        do_kernel_semtimedop(semid, tsops, nsops, timeout, frame.is_from_user())
    }

    fn entry_format(&self, args: &[usize]) -> Vec<FormattedSyscallParam> {
        vec![
            FormattedSyscallParam::new("semid", format!("{}", args[0] as i32)),
            FormattedSyscallParam::new("sops", format!("{:#x}", args[1])),
            FormattedSyscallParam::new("nsops", format!("{}", args[2])),
            FormattedSyscallParam::new("timeout", format!("{:#x}", args[3])),
        ]
    }
}

declare_syscall!(SYS_SEMTIMEDOP, SysSemtimedopHandle);
