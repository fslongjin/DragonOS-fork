use alloc::sync::Arc;
use system_error::SystemError;

use crate::filesystem::vfs::mount::MountFS;
use crate::process::namespace::mount_namespace::init_mount_namespace;

/// 早期VFS初始化完成后的回调，用于设置mount namespace关联
/// 这个函数只应该在VFS初始化时调用一次
pub fn vfs_mount_namespace_init(root_mount_fs: Arc<MountFS>) -> Result<(), SystemError> {
    // 获取根mount namespace
    let root_namespace = init_mount_namespace();

    // 设置根MountFS到namespace中
    root_namespace.set_root_mountfs(root_mount_fs.clone());

    Ok(())
}

/// 更新mount namespace的根文件系统（用于文件系统迁移）
pub fn update_mount_namespace_root(root_mount_fs: Arc<MountFS>) {
    use crate::process::namespace::mount_namespace::init_mount_namespace;
    let root_namespace = init_mount_namespace();
    root_namespace.set_root_mountfs(root_mount_fs);
}
