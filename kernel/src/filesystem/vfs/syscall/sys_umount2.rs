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
use alloc::{sync::Arc, vec::Vec, string::{String, ToString}};
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
    
    // 修复路径构造逻辑：对于绝对路径，直接使用rest；对于相对路径，需要拼接
    let path = if target.starts_with('/') {
        // 绝对路径：直接使用rest，它已经包含了完整路径
        rest.clone()
    } else {
        // 相对路径：需要拼接work的路径和rest
        let work_path = work.absolute_path()?;
        if work_path == "/" {
            format!("/{}", rest)
        } else {
            format!("{}/{}", work_path, rest)
        }
    };

    log::info!("do_umount2: attempting to umount path: {}", path);
    log::info!("do_umount2: original target={}, work.absolute_path()={}, rest={}", target, work.absolute_path()?, rest);

    // 获取当前namespace的挂载列表信息用于调试
    let mount_list = MOUNT_LIST();
    log::info!("do_umount2: current mount list: {:?}", mount_list);

    // 首先尝试从当前namespace的挂载列表中移除
    if let Some(fs) = mount_list.remove(path.clone()) {
        // Todo: 占用检测
        
        // 检查是否是namespace复制的挂载点
        if fs.is_namespace_copy() {
            log::info!("Unmounting namespace copy mount (mount_id: {}): {}", fs.mount_id(), path);
            
            // 对于namespace复制的挂载点，直接处理umount propagation
            if let Some(namespace) = fs.namespace() {
                if let Err(e) = namespace.handle_umount_propagation(&fs, &path) {
                    log::warn!("Failed to handle umount propagation for {}: {:?}", path, e);
                }
            }
            
            // 清理propagation状态
            cleanup_namespace_copy_propagation_state(&fs);
            
            // 对于namespace复制的挂载点，我们不调用MountFS.umount()
            // 因为它没有self_mountpoint，直接返回成功
            log::info!("Successfully unmounted namespace copy mount: {}", path);
            return Ok(fs);
        }
        
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
    
    // 如果直接路径匹配失败，尝试其他路径匹配策略
    log::warn!("do_umount2: direct path match failed for {}, trying alternative matching", path);
    
    // 尝试路径标准化和模糊匹配
    let normalized_path = normalize_umount_path(&path);
    if normalized_path != path {
        log::info!("do_umount2: trying normalized path: {}", normalized_path);
        if let Some(fs) = mount_list.remove(normalized_path.clone()) {
            log::info!("do_umount2: found mount using normalized path: {}", normalized_path);
            
            // 处理卸载
            if fs.is_namespace_copy() {
                if let Some(namespace) = fs.namespace() {
                    if let Err(e) = namespace.handle_umount_propagation(&fs, &normalized_path) {
                        log::warn!("Failed to handle umount propagation for {}: {:?}", normalized_path, e);
                    }
                }
                cleanup_namespace_copy_propagation_state(&fs);
                return Ok(fs);
            } else {
                if let Err(e) = fs.umount() {
                    log::warn!("MountFS.umount() failed for {}: {:?}", normalized_path, e);
                    return Err(e);
                }
                return Ok(fs);
            }
        }
    }
    
    // 尝试模糊匹配：查找包含目标路径的挂载点
    log::info!("do_umount2: trying fuzzy matching for path: {}", path);
    let mount_debug = format!("{:?}", mount_list);
    log::info!("do_umount2: available mounts: {}", mount_debug);
    
    // 最后的尝试：直接通过inode查找挂载点
    let (target_node, rest_path) = user_path_at(&ProcessManager::current_pcb(), dirfd, target)?;
    if rest_path.is_empty() {
        use crate::filesystem::vfs::mount::is_mountpoint_root;
        if is_mountpoint_root(&target_node) {
            log::info!("do_umount2: found mountpoint root inode, attempting direct umount");
            
            // 这是一个挂载点根目录，尝试直接卸载
            match target_node.umount() {
                Ok(mount_fs) => {
                    log::info!("do_umount2: direct inode umount succeeded, got mount_fs id: {}", mount_fs.mount_id());
                    return Ok(mount_fs);
                }
                Err(e) => {
                    log::warn!("do_umount2: direct inode umount failed: {:?}", e);
                    return Err(e);
                }
            }
        }
    }
    
    log::error!("do_umount2: all umount strategies failed for path: {}", path);
    return Err(SystemError::EINVAL);
}

/// 标准化umount路径
fn normalize_umount_path(path: &str) -> String {
    // 移除开头的多个斜杠
    let trimmed = path.trim_start_matches('/');
    
    // 如果路径为空，返回根路径
    if trimmed.is_empty() {
        return "/".to_string();
    }
    
    // 分割路径组件并过滤空组件
    let components: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
    
    // 重新构建路径
    if components.is_empty() {
        "/".to_string()
    } else {
        "/".to_string() + &components.join("/")
    }
}

/// 清理namespace复制挂载点的传播状态
fn cleanup_namespace_copy_propagation_state(mount_fs: &Arc<MountFS>) {
    let prop_info = mount_fs.get_propagation_info();
    
    log::debug!(
        "cleanup_namespace_copy_propagation_state: cleaning up mount_id {}, type: {:?}",
        mount_fs.mount_id(),
        prop_info.prop_type
    );

    if let Some(namespace) = mount_fs.namespace() {
        match prop_info.prop_type {
            crate::process::namespace::mount_namespace::PropagationType::Shared => {
                // 从共享组中移除
                if let Some(group_id) = prop_info.shared_group_id {
                    if let Err(e) = namespace.leave_shared_group(group_id, &Arc::downgrade(mount_fs)) {
                        log::warn!(
                            "cleanup_namespace_copy_propagation_state: failed to leave shared group {}: {:?}",
                            group_id,
                            e
                        );
                    } else {
                        log::info!(
                            "cleanup_namespace_copy_propagation_state: removed mount_id {} from shared group {}",
                            mount_fs.mount_id(),
                            group_id
                        );
                    }
                }
            }
            crate::process::namespace::mount_namespace::PropagationType::Slave => {
                // 清理slave关系
                if let Some(master) = prop_info.master.as_ref().and_then(|w| w.upgrade()) {
                    if let Err(e) = master.remove_slave_mount(mount_fs) {
                        log::warn!(
                            "cleanup_namespace_copy_propagation_state: failed to remove slave mount_id {} from master: {:?}",
                            mount_fs.mount_id(),
                            e
                        );
                    }
                }
                
                // 清理自己的slave list
                for slave_weak in &prop_info.slaves {
                    if let Some(slave) = slave_weak.upgrade() {
                        if let Err(e) = slave.set_master_mount(None) {
                            log::warn!(
                                "cleanup_namespace_copy_propagation_state: failed to clear master for slave mount_id {}: {:?}",
                                slave.mount_id(),
                                e
                            );
                        }
                    }
                }
            }
            crate::process::namespace::mount_namespace::PropagationType::Private |
            crate::process::namespace::mount_namespace::PropagationType::Unbindable => {
                // 私有和不可绑定挂载没有特殊的清理需求
                log::debug!(
                    "cleanup_namespace_copy_propagation_state: mount_id {} has {:?} propagation, no special cleanup needed",
                    mount_fs.mount_id(),
                    prop_info.prop_type
                );
            }
        }
    }
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
