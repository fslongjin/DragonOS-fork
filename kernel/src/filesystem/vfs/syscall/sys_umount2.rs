//! System call handler for sys_umount.

use crate::{
    arch::{interrupt::TrapFrame, syscall::nr::SYS_UMOUNT2},
    filesystem::vfs::{
        fcntl::AtFlags, mount::MOUNT_LIST, utils::user_path_at, MountFS, MAX_PATHLEN,
    },
    process::ProcessManager,
    syscall::{
        table::{FormattedSyscallParam, Syscall},
        user_access,
    },
};
use alloc::{sync::Arc, vec::Vec};
use system_error::SystemError;

/// src/linux/mount.c `umount` & `umount2`
///
/// [umount(2) — Linux manual page](https://www.man7.org/linux/man-pages/man2/umount.2.html)
pub struct SysUmount2Handle;

impl Syscall for SysUmount2Handle {
    fn num_args(&self) -> usize {
        2
    }

    fn handle(&self, args: &[usize], _frame: &mut TrapFrame) -> Result<usize, SystemError> {
        let target = Self::target(args);
        let flags = Self::flags(args);

        let target = user_access::check_and_clone_cstr(target, Some(MAX_PATHLEN))?
            .into_string()
            .map_err(|_| SystemError::EINVAL)?;
        do_umount2(
            AtFlags::AT_FDCWD.bits(),
            &target,
            UmountFlag::from_bits(flags).ok_or(SystemError::EINVAL)?,
        )?;
        return Ok(0);
    }

    fn entry_format(&self, args: &[usize]) -> Vec<FormattedSyscallParam> {
        vec![
            FormattedSyscallParam::new("target", format!("{:#x}", Self::target(args) as usize)),
            FormattedSyscallParam::new("flags", format!("{:#x}", Self::flags(args))),
        ]
    }
}

impl SysUmount2Handle {
    fn target(args: &[usize]) -> *const u8 {
        args[0] as *const u8
    }

    fn flags(args: &[usize]) -> i32 {
        args[1] as i32
    }
}

syscall_table_macros::declare_syscall!(SYS_UMOUNT2, SysUmount2Handle);

/// # do_umount2 - 执行卸载文件系统的函数
///
/// 这个函数用于卸载指定的文件系统。
///
/// ## 参数
///
/// - dirfd: i32 - 目录文件描述符，用于指定要卸载的文件系统的根目录。
/// - target: &str - 要卸载的文件系统的目标路径。
/// - _flag: UmountFlag - 卸载标志，目前未使用。
///
/// ## 返回值
///
/// - Ok(Arc<MountFS>): 成功时返回文件系统的 Arc 引用。
/// - Err(SystemError): 出错时返回系统错误。
///
/// ## 错误处理
///
/// 如果指定的路径没有对应的文件系统，或者在尝试卸载时发生错误，将返回错误。
pub fn do_umount2(
    dirfd: i32,
    target: &str,
    _flag: UmountFlag,
) -> Result<Arc<MountFS>, SystemError> {
    let (work, rest) = user_path_at(&ProcessManager::current_pcb(), dirfd, target)?;
    let path = work.absolute_path()? + &rest;

    // 首先尝试从当前namespace的挂载列表中移除
    if let Some(fs) = MOUNT_LIST().remove(path.clone()) {
        // Todo: 占用检测
        if let Err(e) = fs.umount() {
            // 如果通过MountFS.umount()失败，尝试直接处理
            log::warn!(
                "MountFS.umount() failed for {}: {:?}, trying alternative method",
                path,
                e
            );

            // 对于namespace复制的挂载点，可能self_mountpoint为None
            // 我们通过路径解析来找到对应的inode并进行卸载
            let (target_node, rest_path) =
                user_path_at(&ProcessManager::current_pcb(), dirfd, target)?;
            if rest_path.is_empty() {
                // 直接在目标inode上执行卸载操作，绕过MountFS.umount()的self_mountpoint检查
                use crate::filesystem::vfs::mount::is_mountpoint_root;
                if is_mountpoint_root(&target_node) {
                    // 这是一个挂载点根目录，强制从父文件系统中移除
                    let parent = target_node.parent()?;
                    let metadata = target_node.metadata()?;

                    // 检查父节点是否是MountFSInode，并移除挂载点
                    if let Some(mount_inode) = parent
                        .as_any_ref()
                        .downcast_ref::<crate::filesystem::vfs::mount::MountFSInode>(
                    ) {
                        mount_inode.mount_fs().remove_mountpoint(metadata.inode_id);
                        log::info!("Successfully unmounted {} using alternative method", path);
                        return Ok(fs);
                    }
                }
            }

            return Err(e);
        }
        return Ok(fs);
    }
    return Err(SystemError::EINVAL);
}

bitflags! {
    pub struct UmountFlag: i32 {
        const DEFAULT = 0;          /* Default call to umount. */
        const MNT_FORCE = 1;        /* Force unmounting.  */
        const MNT_DETACH = 2;       /* Just detach from the tree.  */
        const MNT_EXPIRE = 4;       /* Mark for expiry.  */
        const UMOUNT_NOFOLLOW = 8;  /* Don't follow symlink on umount.  */
    }
}
