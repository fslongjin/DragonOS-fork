use alloc::{sync::Arc, vec::Vec};
use system_error::SystemError;

use crate::{
    arch::{interrupt::TrapFrame, syscall::nr::SYS_UNSHARE},
    process::{fork::CloneFlags, ProcessManager},
    syscall::table::{FormattedSyscallParam, Syscall},
};

/// unshare系统调用处理
pub struct SysUnshare;

impl Syscall for SysUnshare {
    fn num_args(&self) -> usize {
        1
    }

    fn handle(&self, args: &[usize], _frame: &mut TrapFrame) -> Result<usize, SystemError> {
        let flags = CloneFlags::from_bits_truncate(args[0] as u64);
        Self::do_unshare(flags)?;
        Ok(0)
    }

    fn entry_format(&self, args: &[usize]) -> Vec<FormattedSyscallParam> {
        vec![FormattedSyscallParam::new(
            "flags",
            format!("{:#x}", args[0]),
        )]
    }
}

impl SysUnshare {
    /// 实现unshare功能
    fn do_unshare(flags: CloneFlags) -> Result<(), SystemError> {
        // 验证标志位
        Self::validate_flags(&flags)?;

        let current_pcb = ProcessManager::current_pcb();

        // 检查是否需要创建新的命名空间
        if !flags.intersects(
            CloneFlags::CLONE_NEWNS
                | CloneFlags::CLONE_NEWUTS
                | CloneFlags::CLONE_NEWIPC
                | CloneFlags::CLONE_NEWPID
                | CloneFlags::CLONE_NEWNET
                | CloneFlags::CLONE_NEWCGROUP
                | CloneFlags::CLONE_NEWTIME
                | CloneFlags::CLONE_NEWUSER,
        ) {
            return Ok(());
        }

        // 对于mount namespace，我们需要特殊处理
        if flags.contains(CloneFlags::CLONE_NEWNS) {
            log::info!(
                "Unshare: creating new mount namespace for pid {:?}",
                current_pcb.raw_pid()
            );
            Self::unshare_mount_namespace(&current_pcb)?;
            log::info!("Unshare: mount namespace created successfully");
        }

        // 处理其他namespace（这里简化实现）
        // TODO: 实现其他namespace的unshare支持

        Ok(())
    }

    /// 验证unshare标志位
    fn validate_flags(flags: &CloneFlags) -> Result<(), SystemError> {
        // 检查不兼容的标志组合

        // CLONE_THREAD和大多数namespace标志不兼容
        if flags.contains(CloneFlags::CLONE_THREAD) {
            if flags.intersects(
                CloneFlags::CLONE_NEWNS
                    | CloneFlags::CLONE_NEWUTS
                    | CloneFlags::CLONE_NEWIPC
                    | CloneFlags::CLONE_NEWNET
                    | CloneFlags::CLONE_NEWUSER,
            ) {
                return Err(SystemError::EINVAL);
            }
        }

        // CLONE_FS和CLONE_NEWNS不兼容
        if flags.contains(CloneFlags::CLONE_FS) && flags.contains(CloneFlags::CLONE_NEWNS) {
            return Err(SystemError::EINVAL);
        }

        // CLONE_SIGHAND需要CLONE_VM
        if flags.contains(CloneFlags::CLONE_SIGHAND) && !flags.contains(CloneFlags::CLONE_VM) {
            return Err(SystemError::EINVAL);
        }

        Ok(())
    }

    /// 为当前进程创建新的mount namespace
    fn unshare_mount_namespace(
        current_pcb: &Arc<crate::process::ProcessControlBlock>,
    ) -> Result<(), SystemError> {
        // 获取当前的mount namespace
        let current_mount_ns = current_pcb.nsproxy().mount_ns.clone();

        // 创建新的mount namespace（复制当前的挂载树）
        let user_ns = current_pcb.cred().user_ns.clone();
        let new_mount_ns = current_mount_ns.create_mount_namespace(user_ns)?;

        // 创建新的nsproxy（复制现有的，但替换mount_ns）
        let current_nsproxy = current_pcb.nsproxy();
        let new_nsproxy = Arc::new(crate::process::namespace::nsproxy::NsProxy {
            pid_ns_for_children: current_nsproxy.pid_ns_for_children.clone(),
            mount_ns: new_mount_ns,
        });

        current_pcb.set_nsproxy(new_nsproxy);

        Ok(())
    }
}

syscall_table_macros::declare_syscall!(SYS_UNSHARE, SysUnshare);
