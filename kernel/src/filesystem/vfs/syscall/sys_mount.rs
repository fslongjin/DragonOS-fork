//! System call handler for sys_mount.

use crate::{
    arch::{interrupt::TrapFrame, syscall::nr::SYS_MOUNT},
    filesystem::vfs::{
        fcntl::AtFlags,
        mount::{mount_flags, MountFS, MountFSInode, MOUNT_LIST},
        produce_fs,
        utils::user_path_at,
        FileSystem, MAX_PATHLEN, VFS_MAX_FOLLOW_SYMLINK_TIMES,
    },
    libs::casting::DowncastArc,
    process::{namespace::mount_namespace::PropagationType, ProcessManager},
    syscall::{
        table::{FormattedSyscallParam, Syscall},
        user_access,
    },
};
use alloc::sync::Arc;
use alloc::vec::Vec;
use system_error::SystemError;

/// #挂载文件系统
///
/// 用于挂载文件系统,目前仅支持ramfs挂载
///
/// ## 参数:
///
/// - source       挂载设备(目前只支持ext4格式的硬盘)
/// - target       挂载目录
/// - filesystemtype   文件系统
/// - mountflags     挂载选项（暂未实现）
/// - data        带数据挂载
///
/// ## 返回值
/// - Ok(0): 挂载成功
/// - Err(SystemError) :挂载过程中出错
pub struct SysMountHandle;

impl Syscall for SysMountHandle {
    fn num_args(&self) -> usize {
        5
    }

    fn handle(&self, args: &[usize], _frame: &mut TrapFrame) -> Result<usize, SystemError> {
        let target = Self::target(args);
        let filesystemtype = Self::filesystemtype(args);
        let data = Self::raw_data(args);
        let source = Self::source(args);
        let mountflags = Self::mountflags(args);

        let target = user_access::check_and_clone_cstr(target, Some(MAX_PATHLEN))?
            .into_string()
            .map_err(|_| SystemError::EINVAL)?;

        // 检查是否是传播类型变更操作
        use crate::filesystem::vfs::mount::mount_flags::*;

        if mountflags & MS_SHARED != 0 {
            return self.handle_propagation_change(&target, PropagationType::Shared, mountflags);
        }
        if mountflags & MS_PRIVATE != 0 {
            return self.handle_propagation_change(&target, PropagationType::Private, mountflags);
        }
        if mountflags & MS_SLAVE != 0 {
            return self.handle_propagation_change(&target, PropagationType::Slave, mountflags);
        }
        if mountflags & MS_UNBINDABLE != 0 {
            return self.handle_propagation_change(
                &target,
                PropagationType::Unbindable,
                mountflags,
            );
        }

        // 常规挂载逻辑
        let source = user_access::check_and_clone_cstr(source, Some(MAX_PATHLEN))?
            .into_string()
            .map_err(|_| SystemError::EINVAL)?;
        let source = source.as_str();

        let fstype_str = user_access::check_and_clone_cstr(filesystemtype, Some(MAX_PATHLEN))?;
        let fstype_str = fstype_str.to_str().map_err(|_| SystemError::EINVAL)?;

        // 检查是否是bind mount
        if source == &target || mountflags & MS_BIND != 0 {
            return self.handle_bind_mount(&source, &target, mountflags);
        }

        let fs = produce_fs(fstype_str, data, source)?;
        do_mount_with_remount_check(fs, &target)?;

        return Ok(0);
    }

    fn entry_format(&self, args: &[usize]) -> Vec<FormattedSyscallParam> {
        vec![
            FormattedSyscallParam::new("source", format!("{:#x}", Self::source(args) as usize)),
            FormattedSyscallParam::new("target", format!("{:#x}", Self::target(args) as usize)),
            FormattedSyscallParam::new(
                "filesystem type",
                format!("{:#x}", Self::filesystemtype(args) as usize),
            ),
            FormattedSyscallParam::new("mountflags", format!("{:#x}", Self::mountflags(args))),
            FormattedSyscallParam::new("data", format!("{:?}", Self::raw_data(args))),
        ]
    }
}

impl SysMountHandle {
    fn source(args: &[usize]) -> *const u8 {
        args[0] as *const u8
    }
    fn target(args: &[usize]) -> *const u8 {
        args[1] as *const u8
    }
    fn filesystemtype(args: &[usize]) -> *const u8 {
        args[2] as *const u8
    }
    fn mountflags(args: &[usize]) -> usize {
        args[3]
    }
    fn raw_data(args: &[usize]) -> Option<&'static str> {
        let raw = args[4] as *const u8;
        if raw.is_null() {
            return None;
        }
        let len = (0..).find(|&i| unsafe { raw.add(i).read() } == 0).unwrap();

        let slice = unsafe { core::slice::from_raw_parts(raw, len) };
        let raw_str = core::str::from_utf8(slice).ok().unwrap();
        Some(raw_str)
    }

    /// 处理传播类型变更
    fn handle_propagation_change(
        &self,
        target: &str,
        prop_type: PropagationType,
        mountflags: usize,
    ) -> Result<usize, SystemError> {
        log::info!(
            "SysMountHandle: handling propagation change to {:?} for {}",
            prop_type,
            target
        );

        let recursive = (mountflags & mount_flags::MS_REC) != 0;

        // 找到目标挂载点
        let current_pcb = ProcessManager::current_pcb();
        let mount_ns = current_pcb.nsproxy().mount_ns.clone();
        let (current_node, rest_path) =
            user_path_at(&current_pcb, AtFlags::AT_FDCWD.bits(), target)?;
        let inode = current_node.lookup_follow_symlink(&rest_path, VFS_MAX_FOLLOW_SYMLINK_TIMES)?;

        // 获取对应的MountFS
        if let Some(mount_fs_inode) = inode.downcast_arc::<MountFSInode>() {
            let mount_fs = mount_fs_inode.mount_fs();
            log::info!(
                "SysMountHandle: found mount_fs with id {}",
                mount_fs.mount_id()
            );
            mount_ns.change_propagation_type(&mount_fs, prop_type, recursive)?;
            log::info!("SysMountHandle: propagation change completed successfully");
        } else {
            log::error!("SysMountHandle: target is not a mount point");
            return Err(SystemError::EINVAL);
        }

        Ok(0)
    }

    /// 处理bind mount
    fn handle_bind_mount(
        &self,
        source: &str,
        target: &str,
        mountflags: usize,
    ) -> Result<usize, SystemError> {
        log::info!(
            "SysMountHandle: handling bind mount from {} to {}",
            source,
            target
        );

        // 找到源挂载点
        let current_pcb = ProcessManager::current_pcb();
        let (source_node, source_rest) =
            user_path_at(&current_pcb, AtFlags::AT_FDCWD.bits(), source)?;
        let source_inode =
            source_node.lookup_follow_symlink(&source_rest, VFS_MAX_FOLLOW_SYMLINK_TIMES)?;

        if let Some(source_mount_inode) = source_inode.downcast_arc::<MountFSInode>() {
            let source_mount = source_mount_inode.mount_fs();

            // 创建bind mount
            let bind_mount = source_mount.create_bind_mount(target, mountflags as u32)?;

            // 找到目标位置并执行挂载
            let (target_node, target_rest) =
                user_path_at(&current_pcb, AtFlags::AT_FDCWD.bits(), target)?;
            let target_inode =
                target_node.lookup_follow_symlink(&target_rest, VFS_MAX_FOLLOW_SYMLINK_TIMES)?;

            target_inode.mount(bind_mount.inner_filesystem())?;

            log::info!("SysMountHandle: bind mount completed successfully");
        } else {
            log::error!("SysMountHandle: source is not a mount point");
            return Err(SystemError::EINVAL);
        }

        Ok(0)
    }
}

syscall_table_macros::declare_syscall!(SYS_MOUNT, SysMountHandle);

/// # do_mount - 挂载文件系统
///
/// 将给定的文件系统挂载到指定的挂载点。
///
/// 此函数会检查是否已经挂载了相同的文件系统，如果已经挂载，则返回错误。
/// 它还会处理符号链接，并确保挂载点是有效的。
///
/// ## 参数
///
/// - `fs`: Arc<dyn FileSystem>，要挂载的文件系统。
/// - `mount_point`: &str，挂载点路径。
///
/// ## 返回值
///
/// - `Ok(Arc<MountFS>)`: 挂载成功后返回挂载的文件系统。
/// - `Err(SystemError)`: 挂载失败时返回错误。

/// 带有重新挂载检查的挂载函数，特别用于处理namespace场景
pub fn do_mount_with_remount_check(
    fs: Arc<dyn FileSystem>,
    mount_point: &str,
) -> Result<Arc<MountFS>, SystemError> {
    let (current_node, rest_path) = user_path_at(
        &ProcessManager::current_pcb(),
        AtFlags::AT_FDCWD.bits(),
        mount_point,
    )?;
    let inode = current_node.lookup_follow_symlink(&rest_path, VFS_MAX_FOLLOW_SYMLINK_TIMES)?;

    // 检查是否已有挂载点
    if let Some((_, rest, existing_fs)) = MOUNT_LIST().get_mount_point(mount_point) {
        if rest.is_empty() {
            // 在namespace环境中，允许在已有挂载点上重新挂载
            // 首先尝试卸载现有的挂载点
            log::info!(
                "Attempting to remount over existing mount point: {}",
                mount_point
            );

            if let Err(umount_err) = super::sys_umount2::do_umount2(
                AtFlags::AT_FDCWD.bits(),
                mount_point,
                super::sys_umount2::UmountFlag::DEFAULT,
            ) {
                log::warn!(
                    "Failed to unmount existing mount point {}: {:?}",
                    mount_point,
                    umount_err
                );

                // 如果卸载失败，检查是否是namespace复制的挂载点（没有挂载点引用的情况）
                if existing_fs.is_namespace_copy() {
                    log::info!("Existing mount point has no self_mountpoint, allowing forced remount in namespace");
                    // 直接从挂载列表中移除并继续挂载
                    MOUNT_LIST().remove(mount_point);
                    // 还需要从父文件系统的mountpoints中移除
                    if let Ok(parent) = inode.parent() {
                        if let Some(parent_mount_inode) =
                            parent
                                .as_any_ref()
                                .downcast_ref::<crate::filesystem::vfs::mount::MountFSInode>()
                        {
                            let metadata = inode.metadata()?;
                            parent_mount_inode
                                .mount_fs()
                                .remove_mountpoint(metadata.inode_id);
                            log::info!(
                                "Removed mountpoint from parent filesystem for forced remount"
                            );
                        }
                    }
                } else {
                    return Err(SystemError::EBUSY);
                }
            }
        }
    }

    // 移至IndexNode.mount()来记录
    return inode.mount(fs);
}

pub fn do_mount(fs: Arc<dyn FileSystem>, mount_point: &str) -> Result<Arc<MountFS>, SystemError> {
    let (current_node, rest_path) = user_path_at(
        &ProcessManager::current_pcb(),
        AtFlags::AT_FDCWD.bits(),
        mount_point,
    )?;
    let inode = current_node.lookup_follow_symlink(&rest_path, VFS_MAX_FOLLOW_SYMLINK_TIMES)?;
    if let Some((_, rest, _fs)) = MOUNT_LIST().get_mount_point(mount_point) {
        if rest.is_empty() {
            return Err(SystemError::EBUSY);
        }
    }
    // 移至IndexNode.mount()来记录
    return inode.mount(fs);
}
