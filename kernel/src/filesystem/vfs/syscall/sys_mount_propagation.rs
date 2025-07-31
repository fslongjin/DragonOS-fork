use alloc::{sync::Arc, vec::Vec};
use system_error::SystemError;

use crate::{
    arch::interrupt::TrapFrame,
    filesystem::vfs::{
        fcntl::AtFlags,
        utils::user_path_at,
        VFS_MAX_FOLLOW_SYMLINK_TIMES,
    },
    process::{namespace::mount_namespace::PropagationType, ProcessManager},
    syscall::{
        table::{FormattedSyscallParam, Syscall},
        user_access::check_and_clone_cstr,
    },
};

// Mount propagation flags - 从Linux兼容
bitflags::bitflags! {
    pub struct MountPropagationFlags: u32 {
        const MS_SHARED     = 1 << 20;  // 设置为shared
        const MS_PRIVATE    = 1 << 18;  // 设置为private  
        const MS_SLAVE      = 1 << 19;  // 设置为slave
        const MS_UNBINDABLE = 1 << 17;  // 设置为unbindable
        
        // 传播标志掩码
        const MS_PROPAGATION = Self::MS_SHARED.bits() | Self::MS_PRIVATE.bits() 
                              | Self::MS_SLAVE.bits() | Self::MS_UNBINDABLE.bits();
    }
}

/// mount --make-shared, --make-private, --make-slave, --make-unbindable 的实现
pub struct SysMountSetPropagation;

impl Syscall for SysMountSetPropagation {
    fn num_args(&self) -> usize {
        3
    }

    fn handle(&self, args: &[usize], _frame: &mut TrapFrame) -> Result<usize, SystemError> {
        let path_ptr = args[0] as *const u8;
        let flags = args[1] as u32;
        let _data = args[2]; // 目前未使用

        let path_cstr = check_and_clone_cstr(path_ptr, Some(4096))?;
        let path = path_cstr.to_str().map_err(|_| SystemError::EINVAL)?;

        Self::do_mount_set_propagation(path, flags)?;
        Ok(0)
    }

    fn entry_format(&self, args: &[usize]) -> Vec<FormattedSyscallParam> {
        vec![
            FormattedSyscallParam::new("path", format!("{:#x}", args[0])),
            FormattedSyscallParam::new("flags", format!("{:#x}", args[1])),
            FormattedSyscallParam::new("data", format!("{:#x}", args[2])),
        ]
    }
}

impl SysMountSetPropagation {
    fn do_mount_set_propagation(path: &str, flags: u32) -> Result<(), SystemError> {
        let propagation_flags = MountPropagationFlags::from_bits(flags & MountPropagationFlags::MS_PROPAGATION.bits())
            .ok_or(SystemError::EINVAL)?;

        // 检查是否只设置了一个传播标志
        let count = propagation_flags.bits().count_ones();
        if count != 1 {
            return Err(SystemError::EINVAL);
        }

        let prop_type = if propagation_flags.contains(MountPropagationFlags::MS_SHARED) {
            PropagationType::Shared
        } else if propagation_flags.contains(MountPropagationFlags::MS_PRIVATE) {
            PropagationType::Private
        } else if propagation_flags.contains(MountPropagationFlags::MS_SLAVE) {
            PropagationType::Slave
        } else if propagation_flags.contains(MountPropagationFlags::MS_UNBINDABLE) {
            PropagationType::Unbindable
        } else {
            return Err(SystemError::EINVAL);
        };

        let pcb = ProcessManager::current_pcb();
        let (inode, rest_path) = user_path_at(&pcb, AtFlags::AT_FDCWD.bits(), path)?;
        let target_inode = inode.lookup_follow_symlink(&rest_path, VFS_MAX_FOLLOW_SYMLINK_TIMES)?;

        // 查找对应的MountFS
        if let Ok(mountfs) = Self::find_mountfs_for_inode(&target_inode) {
            mountfs.set_propagation(prop_type)?;
        } else {
            return Err(SystemError::EINVAL);
        }

        Ok(())
    }

    /// 根据inode查找对应的MountFS
    fn find_mountfs_for_inode(inode: &Arc<dyn crate::filesystem::vfs::IndexNode>) -> Result<Arc<crate::filesystem::vfs::mount::MountFS>, SystemError> {
        use crate::filesystem::vfs::mount::MountFSInode;

        // 检查inode是否是MountFSInode
        if let Some(mount_inode) = inode.as_any_ref().downcast_ref::<MountFSInode>() {
            return Ok(mount_inode.mount_fs());
        }

        // 如果不是MountFSInode，尝试从挂载列表中查找
        // 这里简化实现，返回错误
        Err(SystemError::EINVAL)
    }
}

/// 获取mount propagation属性的系统调用
pub struct SysMountGetPropagation;

impl Syscall for SysMountGetPropagation {
    fn num_args(&self) -> usize {
        1
    }

    fn handle(&self, args: &[usize], _frame: &mut TrapFrame) -> Result<usize, SystemError> {
        let path_ptr = args[0] as *const u8;

        let path_cstr = check_and_clone_cstr(path_ptr, Some(4096))?;
        let path = path_cstr.to_str().map_err(|_| SystemError::EINVAL)?;

        let prop_type = Self::do_mount_get_propagation(path)?;
        
        let flags = match prop_type {
            PropagationType::Shared => MountPropagationFlags::MS_SHARED.bits(),
            PropagationType::Private => MountPropagationFlags::MS_PRIVATE.bits(),
            PropagationType::Slave => MountPropagationFlags::MS_SLAVE.bits(),
            PropagationType::Unbindable => MountPropagationFlags::MS_UNBINDABLE.bits(),
        };

        Ok(flags as usize)
    }

    fn entry_format(&self, args: &[usize]) -> Vec<FormattedSyscallParam> {
        vec![
            FormattedSyscallParam::new("path", format!("{:#x}", args[0])),
        ]
    }
}

impl SysMountGetPropagation {
    fn do_mount_get_propagation(path: &str) -> Result<PropagationType, SystemError> {
        let pcb = ProcessManager::current_pcb();
        let (inode, rest_path) = user_path_at(&pcb, AtFlags::AT_FDCWD.bits(), path)?;
        let target_inode = inode.lookup_follow_symlink(&rest_path, VFS_MAX_FOLLOW_SYMLINK_TIMES)?;

        // 查找对应的MountFS
        if let Ok(mountfs) = SysMountSetPropagation::find_mountfs_for_inode(&target_inode) {
            Ok(mountfs.propagation())
        } else {
            Err(SystemError::EINVAL)
        }
    }
}

// 为了兼容性，可以注册到系统调用表中
// syscall_table_macros::declare_syscall!(SYS_MOUNT_SETATTR, SysMountSetPropagation);
// syscall_table_macros::declare_syscall!(SYS_MOUNT_ATTR, SysMountGetPropagation);