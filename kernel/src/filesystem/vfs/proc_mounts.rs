use alloc::{string::String, sync::Arc, vec::Vec};
use system_error::SystemError;

use crate::{
    filesystem::vfs::{
        file::{FileMode, FilePrivateData},
        FileSystem, FileType, IndexNode, Metadata, PollableInode,
    },
    libs::spinlock::SpinLockGuard,
    process::ProcessManager,
};

/// /proc/mounts的inode实现，显示当前mount namespace的挂载信息
#[derive(Debug)]
pub struct ProcMountsInode {
    metadata: Metadata,
}

impl ProcMountsInode {
    pub fn new() -> Arc<Self> {
        let metadata = Metadata {
            dev_id: 0,
            inode_id: crate::filesystem::vfs::generate_inode_id(),
            size: 0,
            blk_size: 0,
            blocks: 0,
            atime: crate::time::PosixTimeSpec::default(),
            mtime: crate::time::PosixTimeSpec::default(),
            ctime: crate::time::PosixTimeSpec::default(),
            btime: crate::time::PosixTimeSpec::default(),
            file_type: FileType::File,
            mode: crate::filesystem::vfs::syscall::ModeType::S_IRUSR
                | crate::filesystem::vfs::syscall::ModeType::S_IRGRP
                | crate::filesystem::vfs::syscall::ModeType::S_IROTH,
            nlinks: 1,
            uid: 0,
            gid: 0,
            raw_dev: Default::default(),
        };

        Arc::new(Self { metadata })
    }

    /// 生成/proc/mounts的内容
    fn generate_mounts_content() -> String {
        let mut content = String::new();
        
        // 获取当前进程的mount namespace
        let current_pcb = ProcessManager::current_pcb();
        let mount_ns = current_pcb.nsproxy().mount_ns.clone();
        
        // 简化实现：显示基本信息
        content.push_str("# Mount namespace information\n");
        content.push_str("# Device MountPoint FileSystemType Options Dump Pass\n");
        
        // 获取根文件系统信息
        let root_mountfs = mount_ns.root_mountfs();
        let root_fs = root_mountfs.inner_filesystem();
        let root_fs_name = root_fs.name();
        let propagation = root_mountfs.propagation();
        
        content.push_str(&format!(
            "{} / {} rw,{:?} 0 0\n",
            root_fs_name,
            root_fs_name,
            propagation
        ));
        
        if content.is_empty() {
            content.push_str("# No mounts found\n");
        }
        
        content
    }
}

impl IndexNode for ProcMountsInode {
    fn read_at(
        &self,
        offset: usize,
        len: usize,
        buf: &mut [u8],
        _data: SpinLockGuard<FilePrivateData>,
    ) -> Result<usize, SystemError> {
        let content = Self::generate_mounts_content();
        let content_bytes = content.as_bytes();
        
        if offset >= content_bytes.len() {
            return Ok(0);
        }
        
        let end = (offset + len).min(content_bytes.len());
        let read_len = end - offset;
        
        buf[..read_len].copy_from_slice(&content_bytes[offset..end]);
        Ok(read_len)
    }

    fn write_at(
        &self,
        _offset: usize,
        _len: usize,
        _buf: &[u8],
        _data: SpinLockGuard<FilePrivateData>,
    ) -> Result<usize, SystemError> {
        // /proc/mounts是只读的
        Err(SystemError::EPERM)
    }

    fn metadata(&self) -> Result<Metadata, SystemError> {
        let mut metadata = self.metadata.clone();
        // 动态计算大小
        let content = Self::generate_mounts_content();
        metadata.size = content.len() as i64;
        Ok(metadata)
    }

    fn as_any_ref(&self) -> &dyn core::any::Any {
        self
    }

    fn fs(&self) -> Arc<dyn FileSystem> {
        todo!("ProcMountsInode::fs() not implemented")
    }

    fn list(&self) -> Result<Vec<String>, SystemError> {
        Err(SystemError::ENOTDIR)
    }

    fn set_metadata(&self, _metadata: &Metadata) -> Result<(), SystemError> {
        Err(SystemError::EPERM)
    }

    fn resize(&self, _len: usize) -> Result<(), SystemError> {
        Err(SystemError::EPERM)
    }

    fn create_with_data(
        &self,
        _name: &str,
        _file_type: FileType,
        _mode: crate::filesystem::vfs::syscall::ModeType,
        _data: usize,
    ) -> Result<Arc<dyn IndexNode>, SystemError> {
        Err(SystemError::ENOTDIR)
    }

    fn create(
        &self,
        _name: &str,
        _file_type: FileType,
        _mode: crate::filesystem::vfs::syscall::ModeType,
    ) -> Result<Arc<dyn IndexNode>, SystemError> {
        Err(SystemError::ENOTDIR)
    }

    fn link(&self, _name: &str, _other: &Arc<dyn IndexNode>) -> Result<(), SystemError> {
        Err(SystemError::EPERM)
    }

    fn unlink(&self, _name: &str) -> Result<(), SystemError> {
        Err(SystemError::EPERM)
    }

    fn rmdir(&self, _name: &str) -> Result<(), SystemError> {
        Err(SystemError::ENOTDIR)
    }

    fn move_to(
        &self,
        _old_name: &str,
        _target: &Arc<dyn IndexNode>,
        _new_name: &str,
    ) -> Result<(), SystemError> {
        Err(SystemError::EPERM)
    }

    fn find(&self, _name: &str) -> Result<Arc<dyn IndexNode>, SystemError> {
        Err(SystemError::ENOTDIR)
    }

    fn get_entry_name(&self, _ino: crate::filesystem::vfs::InodeId) -> Result<String, SystemError> {
        Err(SystemError::ENOTDIR)
    }

    fn get_entry_name_and_metadata(
        &self,
        _ino: crate::filesystem::vfs::InodeId,
    ) -> Result<(String, Metadata), SystemError> {
        Err(SystemError::ENOTDIR)
    }

    fn ioctl(
        &self,
        _cmd: u32,
        _data: usize,
        _private_data: &FilePrivateData,
    ) -> Result<usize, SystemError> {
        Err(SystemError::ENOTTY)
    }

    fn truncate(&self, _len: usize) -> Result<(), SystemError> {
        Err(SystemError::EPERM)
    }

    fn sync(&self) -> Result<(), SystemError> {
        Ok(())
    }

    fn open(
        &self,
        _data: SpinLockGuard<FilePrivateData>,
        _mode: &FileMode,
    ) -> Result<(), SystemError> {
        Ok(())
    }

    fn close(&self, _data: SpinLockGuard<FilePrivateData>) -> Result<(), SystemError> {
        Ok(())
    }

    fn read_direct(
        &self,
        offset: usize,
        len: usize,
        buf: &mut [u8],
        data: SpinLockGuard<FilePrivateData>,
    ) -> Result<usize, SystemError> {
        self.read_at(offset, len, buf, data)
    }

    fn write_direct(
        &self,
        _offset: usize,
        _len: usize,
        _buf: &[u8],
        _data: SpinLockGuard<FilePrivateData>,
    ) -> Result<usize, SystemError> {
        Err(SystemError::EPERM)
    }

    fn mount(&self, _fs: Arc<dyn FileSystem>) -> Result<Arc<crate::filesystem::vfs::mount::MountFS>, SystemError> {
        Err(SystemError::ENOTDIR)
    }

    fn mount_from(&self, _from: Arc<dyn IndexNode>) -> Result<Arc<crate::filesystem::vfs::mount::MountFS>, SystemError> {
        Err(SystemError::ENOTDIR)
    }

    fn umount(&self) -> Result<Arc<crate::filesystem::vfs::mount::MountFS>, SystemError> {
        Err(SystemError::EINVAL)
    }

    fn absolute_path(&self) -> Result<String, SystemError> {
        Ok(String::from("/proc/mounts"))
    }

    fn mknod(
        &self,
        _filename: &str,
        _mode: crate::filesystem::vfs::syscall::ModeType,
        _dev_t: crate::driver::base::device::device_number::DeviceNumber,
    ) -> Result<Arc<dyn IndexNode>, SystemError> {
        Err(SystemError::EPERM)
    }

    fn special_node(&self) -> Option<crate::filesystem::vfs::SpecialNodeData> {
        None
    }

    fn dname(&self) -> Result<crate::filesystem::vfs::utils::DName, SystemError> {
        Ok(crate::filesystem::vfs::utils::DName(Arc::new(String::from("mounts"))))
    }

    fn parent(&self) -> Result<Arc<dyn IndexNode>, SystemError> {
        Err(SystemError::ENOENT)
    }

    fn page_cache(&self) -> Option<Arc<crate::filesystem::page_cache::PageCache>> {
        None
    }

    fn as_pollable_inode(&self) -> Result<&dyn PollableInode, SystemError> {
        Err(SystemError::ENOSYS)
    }
}