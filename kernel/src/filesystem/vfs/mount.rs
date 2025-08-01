use core::{
    any::Any,
    fmt::Debug,
    sync::atomic::{compiler_fence, Ordering},
};

use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    sync::{Arc, Weak},
    vec::Vec,
};
use system_error::SystemError;

/// Mount flags for mount propagation
pub mod mount_flags {
    /// 创建一个私有挂载，默认行为
    pub const MS_PRIVATE: usize = 1 << 18;
    /// 创建一个共享挂载
    pub const MS_SHARED: usize = 1 << 20;
    /// 创建一个从属挂载
    pub const MS_SLAVE: usize = 1 << 19;
    /// 创建一个不可绑定挂载
    pub const MS_UNBINDABLE: usize = 1 << 17;

    /// 递归标志，用于递归地设置传播类型
    pub const MS_REC: usize = 16384;

    /// Bind mount标志
    pub const MS_BIND: usize = 4096;
}

use crate::{
    driver::base::device::device_number::DeviceNumber,
    filesystem::{
        page_cache::PageCache,
        vfs::{fcntl::AtFlags, vcore::do_mkdir_at, ROOT_INODE},
    },
    libs::{
        casting::DowncastArc,
        rwlock::RwLock,
        spinlock::{SpinLock, SpinLockGuard},
    },
    mm::{fault::PageFaultMessage, VmFaultReason},
};

use super::{
    file::FileMode, syscall::ModeType, utils::DName, FilePrivateData, FileSystem, FileType,
    IndexNode, InodeId, Magic, PollableInode, SuperBlock,
};

const MOUNTFS_BLOCK_SIZE: u64 = 512;
const MOUNTFS_MAX_NAMELEN: u64 = 64;
/// @brief 挂载文件系统
/// 挂载文件系统的时候，套了MountFS这一层，以实现文件系统的递归挂载
#[derive(Debug)]
pub struct MountFS {
    // MountFS内部的文件系统
    inner_filesystem: Arc<dyn FileSystem>,
    /// 用来存储InodeID->挂载点的MountFS的B树
    mountpoints: SpinLock<BTreeMap<InodeId, Arc<MountFS>>>,
    /// 当前文件系统挂载到的那个挂载点的Inode
    self_mountpoint: Option<Arc<MountFSInode>>,
    /// 指向当前MountFS的弱引用
    self_ref: Weak<MountFS>,

    /// === 新增字段：namespace支持 ===
    /// 所属的mount namespace（使用RwLock提供内部可变性）
    namespace: RwLock<Weak<crate::process::namespace::mount_namespace::MountNamespace>>,
    /// propagation信息
    propagation: RwLock<crate::process::namespace::mount_namespace::MountPropagation>,
    /// 挂载ID（用于内核调试和/proc输出）
    mount_id: u32,
}

/// @brief MountFS的Index Node 注意，这个IndexNode只是一个中间层。它的目的是将具体文件系统的Inode与挂载机制连接在一起。
#[derive(Debug)]
#[cast_to([sync] IndexNode)]
pub struct MountFSInode {
    /// 当前挂载点对应到具体的文件系统的Inode
    inner_inode: Arc<dyn IndexNode>,
    /// 当前Inode对应的MountFS
    mount_fs: Arc<MountFS>,
    /// 指向自身的弱引用
    self_ref: Weak<MountFSInode>,
}

impl MountFS {
    /// 新的构造函数，支持namespace
    pub fn new_with_namespace(
        inner_filesystem: Arc<dyn FileSystem>,
        self_mountpoint: Option<Arc<MountFSInode>>,
        namespace: Weak<crate::process::namespace::mount_namespace::MountNamespace>,
        propagation: crate::process::namespace::mount_namespace::MountPropagation,
        mount_id: u32,
    ) -> Arc<Self> {
        Arc::new_cyclic(|self_ref| MountFS {
            inner_filesystem,
            mountpoints: SpinLock::new(BTreeMap::new()),
            self_mountpoint,
            self_ref: self_ref.clone(),
            namespace: RwLock::new(namespace),
            propagation: RwLock::new(propagation),
            mount_id,
        })
    }

    /// 保持向后兼容的构造函数（启动时使用，不依赖namespace）
    pub fn new(
        inner_filesystem: Arc<dyn FileSystem>,
        self_mountpoint: Option<Arc<MountFSInode>>,
    ) -> Arc<Self> {
        use crate::process::namespace::mount_namespace::MountPropagation;

        // 启动时的简化版本，不依赖mount namespace避免循环依赖
        Self::new_with_namespace(
            inner_filesystem,
            self_mountpoint,
            Weak::new(), // 空的namespace引用
            MountPropagation::new_private(),
            0, // 默认mount_id
        )
    }

    /// @brief 用Arc指针包裹MountFS对象。
    /// 本函数的主要功能为，初始化MountFS对象中的自引用Weak指针
    /// 本函数只应在构造器中被调用
    #[allow(dead_code)]
    #[deprecated]
    fn wrap(self) -> Arc<Self> {
        // 创建Arc指针
        let mount_fs: Arc<MountFS> = Arc::new(self);
        // 创建weak指针
        let weak: Weak<MountFS> = Arc::downgrade(&mount_fs);

        // 将Arc指针转为Raw指针并对其内部的self_ref字段赋值
        let ptr: *mut MountFS = mount_fs.as_ref() as *const Self as *mut Self;
        unsafe {
            (*ptr).self_ref = weak;
            // 返回初始化好的MountFS对象
            return mount_fs;
        }
    }

    /// @brief 获取挂载点的文件系统的root inode
    pub fn mountpoint_root_inode(&self) -> Arc<MountFSInode> {
        return Arc::new_cyclic(|self_ref| MountFSInode {
            inner_inode: self.inner_filesystem.root_inode(),
            mount_fs: self.self_ref.upgrade().unwrap(),
            self_ref: self_ref.clone(),
        });
    }

    pub fn inner_filesystem(&self) -> Arc<dyn FileSystem> {
        return self.inner_filesystem.clone();
    }

    pub fn self_ref(&self) -> Arc<Self> {
        self.self_ref.upgrade().unwrap()
    }

    /// 为新的mount namespace深度复制当前MountFS
    /// 创建一个新的MountFS实例，具有相同的内部文件系统但独立的挂载点管理
    pub fn deep_copy_for_namespace(
        &self,
        new_namespace: Weak<crate::process::namespace::mount_namespace::MountNamespace>,
        mount_id: u32,
    ) -> Arc<Self> {
        // 创建新的MountFS
        let new_mount_fs = Self::new_with_namespace(
            self.inner_filesystem.clone(),
            None, // namespace根挂载点的self_mountpoint确实应该是None
            new_namespace.clone(),
            crate::process::namespace::mount_namespace::MountPropagation::new_private(),
            mount_id,
        );

        // 递归复制所有子挂载点
        let source_mountpoints = self.mountpoints.lock();
        let mut new_mountpoints = new_mount_fs.mountpoints.lock();

        for (inode_id, child_mount_fs) in source_mountpoints.iter() {
            // 递归复制子挂载点，为每个子挂载点分配新的全局mount ID
            let new_child = child_mount_fs.deep_copy_for_namespace(
                new_namespace.clone(),
                crate::process::namespace::mount_namespace::alloc_global_mount_id(),
            );
            new_mountpoints.insert(*inode_id, new_child);
        }

        drop(new_mountpoints);
        drop(source_mountpoints);

        new_mount_fs
    }

    /// 获取挂载ID
    pub fn mount_id(&self) -> u32 {
        self.mount_id
    }

    /// 检查是否是namespace复制的挂载点（没有self_mountpoint引用）
    pub fn is_namespace_copy(&self) -> bool {
        self.self_mountpoint.is_none()
    }

    /// 移除子挂载点（用于强制卸载）
    pub fn remove_mountpoint(&self, inode_id: InodeId) -> Option<Arc<MountFS>> {
        self.mountpoints.lock().remove(&inode_id)
    }

    /// 获取子挂载点列表（用于传播引擎）
    pub fn get_child_mounts(&self) -> Vec<(InodeId, Arc<MountFS>)> {
        let mountpoints = self.mountpoints.lock();
        mountpoints
            .iter()
            .map(|(id, mount)| (*id, mount.clone()))
            .collect()
    }

    /// 添加从属挂载（用于master-slave关系）
    pub fn add_slave_mount(&self, slave: Arc<MountFS>) -> Result<(), SystemError> {
        let mut prop = self.propagation.write();
        prop.add_slave(Arc::downgrade(&slave));
        Ok(())
    }

    /// 移除从属挂载
    pub fn remove_slave_mount(&self, slave: &Arc<MountFS>) -> Result<(), SystemError> {
        let mut prop = self.propagation.write();
        prop.remove_slave(&Arc::downgrade(slave));
        Ok(())
    }

    /// 设置master挂载（用于slave挂载）
    pub fn set_master_mount(&self, master: Option<Arc<MountFS>>) -> Result<(), SystemError> {
        let mut prop = self.propagation.write();
        prop.master = master.map(|m| Arc::downgrade(&m));
        Ok(())
    }

    /// 设置共享组ID
    pub fn set_shared_group_id(&self, group_id: Option<u32>) -> Result<(), SystemError> {
        let mut prop = self.propagation.write();
        prop.shared_group_id = group_id;
        prop.peer_group_id = group_id;
        Ok(())
    }

    /// 创建bind mount
    pub fn create_bind_mount(
        &self,
        target_path: &str,
        flags: u32,
    ) -> Result<Arc<MountFS>, SystemError> {
        log::info!(
            "MountFS: creating bind mount from mount_id {} to {}",
            self.mount_id(),
            target_path
        );

        // 检查是否为unbindable
        if self.propagation()
            == crate::process::namespace::mount_namespace::PropagationType::Unbindable
        {
            log::error!("MountFS: cannot bind mount unbindable filesystem");
            return Err(SystemError::EINVAL);
        }

        // 获取当前的namespace
        let namespace = self.namespace().ok_or(SystemError::EINVAL)?;

        // 分配新的mount ID
        let mount_id = crate::process::namespace::mount_namespace::alloc_global_mount_id();

        // 创建新的MountFS，共享相同的文件系统
        let bind_mount = MountFS::new_with_namespace(
            self.inner_filesystem.clone(),
            None, // 将在实际挂载时设置
            Arc::downgrade(&namespace),
            self.get_propagation_info(), // 继承传播属性
            mount_id,
        );

        // 处理传播标志
        let flags = flags as usize; // 转换为usize以匹配mount_flags常量
        if flags & mount_flags::MS_SHARED != 0 {
            bind_mount.set_propagation(
                crate::process::namespace::mount_namespace::PropagationType::Shared,
            )?;
        } else if flags & mount_flags::MS_PRIVATE != 0 {
            bind_mount.set_propagation(
                crate::process::namespace::mount_namespace::PropagationType::Private,
            )?;
        } else if flags & mount_flags::MS_SLAVE != 0 {
            bind_mount.set_propagation(
                crate::process::namespace::mount_namespace::PropagationType::Slave,
            )?;
            // 建立master-slave关系
            namespace.establish_master_slave_relationship(
                &self.self_ref.upgrade().unwrap(),
                &bind_mount,
            )?;
        } else if flags & mount_flags::MS_UNBINDABLE != 0 {
            bind_mount.set_propagation(
                crate::process::namespace::mount_namespace::PropagationType::Unbindable,
            )?;
        }

        // 处理bind mount传播
        namespace.handle_bind_mount(&bind_mount, target_path, flags as u32)?;

        log::info!(
            "MountFS: bind mount created with mount_id {}",
            bind_mount.mount_id()
        );
        Ok(bind_mount)
    }

    /// 检查两个MountFS是否在同一个共享组
    pub fn is_in_same_shared_group(&self, other: &Arc<MountFS>) -> bool {
        let self_prop = self.get_propagation_info();
        let other_prop = other.get_propagation_info();

        match (self_prop.shared_group_id, other_prop.shared_group_id) {
            (Some(self_group), Some(other_group)) => self_group == other_group,
            _ => false,
        }
    }

    /// 检查是否是另一个挂载的slave
    pub fn is_slave_of(&self, potential_master: &Arc<MountFS>) -> bool {
        let prop_info = self.get_propagation_info();

        if let Some(master_weak) = &prop_info.master {
            if let Some(master) = master_weak.upgrade() {
                return Arc::ptr_eq(&master, potential_master);
            }
        }

        false
    }

    /// 获取所有slave挂载
    pub fn get_slave_mounts(&self) -> Vec<Arc<MountFS>> {
        let prop_info = self.get_propagation_info();
        prop_info
            .slaves
            .iter()
            .filter_map(|weak| weak.upgrade())
            .collect()
    }

    /// 清理过期的slave引用
    pub fn cleanup_stale_slaves(&self) -> Result<(), SystemError> {
        let mut prop = self.propagation.write();
        prop.cleanup_stale_slaves();
        Ok(())
    }

    /// 检查是否是bind mount（共享相同的底层文件系统）
    pub fn is_bind_mount_of(&self, other: &Arc<MountFS>) -> bool {
        // 比较底层文件系统的指针
        Arc::ptr_eq(&self.inner_filesystem, &other.inner_filesystem)
    }

    /// 获取bind mount的源挂载（如果是bind mount的话）
    pub fn get_bind_source(&self) -> Option<Arc<MountFS>> {
        // 这里需要通过某种方式追踪bind mount的源
        // 在实际实现中，可能需要在MountFS中添加源引用字段
        // 目前简化实现，返回None
        None
    }

    /// 检查挂载是否支持bind mount
    pub fn supports_bind_mount(&self) -> bool {
        // 检查传播类型是否允许bind mount
        !matches!(
            self.propagation(),
            crate::process::namespace::mount_namespace::PropagationType::Unbindable
        )
    }

    /// 插入子挂载点（用于传播）
    pub fn insert_mountpoint(&self, inode_id: InodeId, mount_fs: Arc<MountFS>) {
        self.mountpoints.lock().insert(inode_id, mount_fs);
    }

    /// 创建挂载的完整路径信息字符串（用于调试）
    pub fn get_mount_info_string(&self) -> alloc::string::String {
        let prop_info = self.get_propagation_info();
        let namespace = self.namespace();

        let mut info = alloc::format!(
            "mount_id: {}, fs: {}, prop: {:?}",
            self.mount_id(),
            self.inner_filesystem.name(),
            prop_info.prop_type
        );

        if let Some(group_id) = prop_info.shared_group_id {
            info.push_str(&alloc::format!(", shared_group: {}", group_id));
        }

        if let Some(_ns) = namespace {
            // 简化实现，避免导入额外的trait
            info.push_str(", has_namespace: true");
        }

        if !prop_info.slaves.is_empty() {
            info.push_str(&alloc::format!(", slaves: {}", prop_info.slaves.len()));
        }

        info
    }

    /// 获取所属的namespace
    pub fn namespace(
        &self,
    ) -> Option<Arc<crate::process::namespace::mount_namespace::MountNamespace>> {
        self.namespace.read().upgrade()
    }

    /// 设置namespace引用（用于创建时的循环引用设置）
    pub fn set_namespace(
        &self,
        namespace: Weak<crate::process::namespace::mount_namespace::MountNamespace>,
    ) {
        let mut ns_guard = self.namespace.write();
        // 为了安全起见，我们只在特定条件下允许这种操作
        // 比如当前namespace为空时（即初始化阶段）
        if ns_guard.upgrade().is_none() {
            *ns_guard = namespace;
            log::info!(
                "set_namespace: successfully set namespace for mount_fs id: {}",
                self.mount_id
            );
        } else {
            log::warn!(
                "set_namespace: attempted to change existing namespace for mount_fs id: {}",
                self.mount_id
            );
            log::warn!("Attempting to set namespace on existing MountFS - consider using new_with_namespace instead");
        }
    }

    /// 设置propagation类型
    pub fn set_propagation(
        &self,
        prop_type: crate::process::namespace::mount_namespace::PropagationType,
    ) -> Result<(), SystemError> {
        use crate::process::namespace::mount_namespace::PropagationType;

        log::info!(
            "MountFS: setting propagation type to {:?} for mount_id {}",
            prop_type,
            self.mount_id()
        );

        let old_type = {
            let prop = self.propagation.read();
            prop.prop_type
        };

        if old_type == prop_type {
            log::debug!("MountFS: propagation type unchanged, skipping");
            return Ok(());
        }

        let mut prop = self.propagation.write();

        // 清理旧的传播状态
        match old_type {
            PropagationType::Shared => {
                // 退出共享组
                if let Some(group_id) = prop.shared_group_id.take() {
                    drop(prop); // 释放锁以避免死锁
                    let ns = self.namespace.read().upgrade();
                    if let Some(ns) = ns {
                        ns.leave_shared_group(group_id, &self.self_ref)?;
                    }
                    prop = self.propagation.write(); // 重新获取锁
                }
            }
            PropagationType::Slave => {
                // 清除master引用，在master中移除自己
                if let Some(master_weak) = prop.master.take() {
                    if let Some(master) = master_weak.upgrade() {
                        drop(prop); // 释放锁
                        master.remove_slave_mount(&self.self_ref.upgrade().unwrap())?;
                        prop = self.propagation.write(); // 重新获取锁
                    }
                }
            }
            _ => {
                // Private和Unbindable无需特殊清理
            }
        }

        // 设置新的传播状态
        match prop_type {
            PropagationType::Shared => {
                // 加入或创建共享组
                drop(prop); // 释放锁
                let ns = self.namespace.read().upgrade();
                if let Some(ns) = ns {
                    let group_id = ns.create_or_join_shared_group(self.self_ref.clone())?;
                    prop = self.propagation.write(); // 重新获取锁
                    prop.shared_group_id = Some(group_id);
                    prop.peer_group_id = Some(group_id);
                } else {
                    prop = self.propagation.write(); // 重新获取锁
                }

                // 清除master引用（shared不能有master）
                prop.master = None;
            }
            PropagationType::Private => {
                // 已在上面清理过共享组，这里只需清除其他状态
                prop.shared_group_id = None;
                prop.peer_group_id = None;
                prop.master = None;
                // 清空slaves列表
                prop.slaves.clear();
            }
            PropagationType::Slave => {
                // Slave类型需要有master，但这里只是设置类型
                // master会通过其他方法设置
                prop.shared_group_id = None;
                prop.peer_group_id = None;
                // master在establish_master_slave_relationship中设置
            }
            PropagationType::Unbindable => {
                // 不可绑定类型
                prop.shared_group_id = None;
                prop.peer_group_id = None;
                prop.master = None;
                prop.slaves.clear();
            }
        }

        prop.prop_type = prop_type;
        log::info!(
            "MountFS: propagation type changed from {:?} to {:?} for mount_id {}",
            old_type,
            prop_type,
            self.mount_id()
        );
        Ok(())
    }

    /// 获取propagation信息
    pub fn propagation(&self) -> crate::process::namespace::mount_namespace::PropagationType {
        self.propagation.read().prop_type
    }

    /// 获取完整的propagation信息
    pub fn get_propagation_info(
        &self,
    ) -> crate::process::namespace::mount_namespace::MountPropagation {
        let prop = self.propagation.read();
        prop.clone()
    }

    /// 卸载文件系统
    /// # Errors
    /// 如果当前文件系统是根文件系统，那么将会返回`EINVAL`
    pub fn umount(&self) -> Result<Arc<MountFS>, SystemError> {
        if let Some(mountpoint) = &self.self_mountpoint {
            // 正常情况：从挂载点卸载
            mountpoint.do_umount()
        } else {
            // 处理namespace复制的挂载点：没有self_mountpoint
            // 这种情况下，我们不能通过do_umount()来卸载，因为它需要从父文件系统中移除挂载点
            // 对于namespace复制的挂载点，应该通过其他方式处理
            log::warn!("MountFS::umount() called on namespace copy mount (mount_id: {}), this should be handled by do_umount2", self.mount_id);
            Err(SystemError::EINVAL)
        }
    }
}

impl MountFSInode {
    /// 获取对应的MountFS
    pub fn mount_fs(&self) -> Arc<MountFS> {
        self.mount_fs.clone()
    }

    /// 清理挂载的传播状态
    fn cleanup_mount_propagation_state_for_mount(mount_fs: &Arc<MountFS>) -> Result<(), SystemError> {
        let prop_info = mount_fs.get_propagation_info();
        
        log::debug!(
            "MountFSInode: cleaning up propagation state for mount_id {}, type: {:?}",
            mount_fs.mount_id(),
            prop_info.prop_type
        );

        if let Some(namespace) = mount_fs.namespace() {
            match prop_info.prop_type {
                crate::process::namespace::mount_namespace::PropagationType::Shared => {
                    // 从共享组中移除
                    if let Some(group_id) = prop_info.shared_group_id {
                        namespace.leave_shared_group(group_id, &Arc::downgrade(mount_fs))?;
                        log::info!(
                            "MountFSInode: removed mount_id {} from shared group {}",
                            mount_fs.mount_id(),
                            group_id
                        );
                    }
                }
                crate::process::namespace::mount_namespace::PropagationType::Slave => {
                    // 清理slave关系
                    if let Some(master) = prop_info.master.as_ref().and_then(|w| w.upgrade()) {
                        if let Err(e) = master.remove_slave_mount(mount_fs) {
                            log::warn!(
                                "MountFSInode: failed to remove slave mount_id {} from master: {:?}",
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
                                    "MountFSInode: failed to clear master for slave mount_id {}: {:?}",
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
                        "MountFSInode: mount_id {} has {:?} propagation, no special cleanup needed",
                        mount_fs.mount_id(),
                        prop_info.prop_type
                    );
                }
            }
        }

        Ok(())
    }

    /// 在指定的mount namespace中找到包含当前inode的MountFS
    /// 通过路径解析来找到正确的MountFS
    fn find_namespace_mountfs(
        &self,
        mount_ns: &Arc<crate::process::namespace::mount_namespace::MountNamespace>,
    ) -> Result<Arc<MountFS>, SystemError> {
        // 获取当前inode的绝对路径
        let target_path = self.absolute_path()?;
        log::info!("find_namespace_mountfs: target_path = {:?}", target_path);

        // 从namespace的root开始，沿路径查找包含此inode的MountFS
        let root_mountfs = mount_ns.root_mountfs();

        // 这里我们需要实现路径解析逻辑
        // 但是考虑到DragonOS当前的架构，最直接的方法是：
        // 1. 如果当前inode就在root文件系统上，返回root MountFS
        // 2. 否则，查找mount list中是否有匹配的挂载点

        let mount_list = mount_ns.mount_list();
        let mount_map = mount_list.0.read();

        // 检查是否有挂载点包含或等于当前路径
        let mut best_match: Option<Arc<MountFS>> = None;
        let mut best_match_len = 0;

        for (mount_path, mount_fs) in mount_map.iter() {
            let mount_path_str = mount_path.to_string();
            log::info!(
                "find_namespace_mountfs: checking mount_path = {}",
                mount_path_str
            );

            // 检查当前路径是否以mount_path开头（即在这个挂载点下）
            if target_path.as_str().starts_with(&mount_path_str) {
                // 找到更长的匹配（更具体的挂载点）
                if mount_path_str.len() > best_match_len {
                    best_match = Some(mount_fs.clone());
                    best_match_len = mount_path_str.len();
                    log::info!(
                        "find_namespace_mountfs: found better match: {} (len={})",
                        mount_path_str,
                        best_match_len
                    );
                }
            }
        }

        // 如果找到了更具体的挂载点，使用它；否则使用root
        if let Some(mount_fs) = best_match {
            log::info!(
                "find_namespace_mountfs: using best match mount_fs id: {}",
                mount_fs.mount_id()
            );
            Ok(mount_fs)
        } else {
            log::info!(
                "find_namespace_mountfs: using root mount_fs id: {}",
                root_mountfs.mount_id()
            );
            Ok(root_mountfs)
        }
    }
    /// @brief 用Arc指针包裹MountFSInode对象。
    /// 本函数的主要功能为，初始化MountFSInode对象中的自引用Weak指针
    /// 本函数只应在构造器中被调用
    #[allow(dead_code)]
    #[deprecated]
    fn wrap(self) -> Arc<Self> {
        // 创建Arc指针
        let inode: Arc<MountFSInode> = Arc::new(self);
        // 创建Weak指针
        let weak: Weak<MountFSInode> = Arc::downgrade(&inode);
        // 将Arc指针转为Raw指针并对其内部的self_ref字段赋值
        compiler_fence(Ordering::SeqCst);
        let ptr: *mut MountFSInode = inode.as_ref() as *const Self as *mut Self;
        compiler_fence(Ordering::SeqCst);
        unsafe {
            (*ptr).self_ref = weak;
            compiler_fence(Ordering::SeqCst);

            // 返回初始化好的MountFSInode对象
            return inode;
        }
    }

    /// @brief 判断当前inode是否为它所在的文件系统的root inode
    fn is_mountpoint_root(&self) -> Result<bool, SystemError> {
        return Ok(self.inner_inode.fs().root_inode().metadata()?.inode_id
            == self.inner_inode.metadata()?.inode_id);
    }

    /// @brief 在挂载树上进行inode替换。
    /// 如果当前inode是父MountFS内的一个挂载点，那么，本函数将会返回挂载到这个挂载点下的文件系统的root inode.
    /// 如果当前inode在父MountFS内，但不是挂载点，那么说明在这里不需要进行inode替换，因此直接返回当前inode。
    ///
    /// 现在支持mount namespace感知
    ///
    /// @return Arc<MountFSInode>
    fn overlaid_inode(&self) -> Arc<MountFSInode> {
        let inode_id = self.metadata().unwrap().inode_id;

        // Namespace感知的MountFS查找逻辑
        let target_mount_fs = if crate::process::ProcessManager::initialized() {
            let current_pcb = crate::process::ProcessManager::current_pcb();
            let mount_ns = current_pcb.nsproxy().mount_ns.clone();

            // 检查当前mount_fs是否属于当前namespace
            if let Some(current_ns) = self.mount_fs.namespace() {
                if Arc::ptr_eq(&current_ns, &mount_ns) {
                    // 当前mount_fs属于当前namespace，直接使用
                    log::info!(
                        "overlaid_inode: mount_fs id {} belongs to current namespace",
                        self.mount_fs.mount_id()
                    );
                    self.mount_fs.clone()
                } else {
                    // mount_fs属于其他namespace，需要找到当前namespace中对应的MountFS
                    log::info!("overlaid_inode: mount_fs id {} belongs to different namespace, finding corresponding one", self.mount_fs.mount_id());
                    self.find_corresponding_mountfs_in_namespace(&mount_ns)
                }
            } else {
                // mount_fs没有namespace，可能是旧的实例，使用当前namespace的root
                log::info!(
                    "overlaid_inode: mount_fs has no namespace, using current namespace root"
                );
                mount_ns.root_mountfs()
            }
        } else {
            // 进程管理未初始化，使用原来的逻辑
            self.mount_fs.clone()
        };

        // 检查mountpoints
        let mountpoints = target_mount_fs.mountpoints.lock();
        if let Some(sub_mountfs) = mountpoints.get(&inode_id) {
            log::info!(
                "overlaid_inode: found mountpoint for inode_id {:?}, jumping to mount_fs id: {}",
                inode_id,
                sub_mountfs.mount_id()
            );
            let result = sub_mountfs.mountpoint_root_inode();
            drop(mountpoints);
            return result;
        } else {
            log::info!(
                "overlaid_inode: no mountpoint found for inode_id {:?}, using self",
                inode_id
            );
            drop(mountpoints);
            return self.self_ref.upgrade().unwrap();
        }
    }

    /// 在指定namespace中找到与当前mount_fs对应的MountFS实例
    fn find_corresponding_mountfs_in_namespace(
        &self,
        mount_ns: &Arc<crate::process::namespace::mount_namespace::MountNamespace>,
    ) -> Arc<MountFS> {
        let current_mount_id = self.mount_fs.mount_id();

        // 首先检查是否是root文件系统
        let ns_root = mount_ns.root_mountfs();
        if current_mount_id == 0 || ns_root.mount_id() == current_mount_id {
            log::info!(
                "find_corresponding_mountfs: using namespace root mount_fs id: {}",
                ns_root.mount_id()
            );
            return ns_root;
        }

        // 检查mount list中是否有相同mount_id的MountFS
        let mount_list = mount_ns.mount_list();
        let mount_map = mount_list.0.read();
        for (_, mount_fs) in mount_map.iter() {
            if mount_fs.mount_id() == current_mount_id {
                log::info!(
                    "find_corresponding_mountfs: found matching mount_fs id: {}",
                    current_mount_id
                );
                return mount_fs.clone();
            }
        }

        // 如果找不到匹配的，使用namespace的root
        log::warn!("find_corresponding_mountfs: no matching mount_fs found, using namespace root");
        ns_root
    }

    fn do_find(&self, name: &str) -> Result<Arc<MountFSInode>, SystemError> {
        // 直接调用当前inode所在的文件系统的find方法进行查找
        // 由于向下查找可能会跨越文件系统的边界，因此需要尝试替换inode
        let inner_inode = self.inner_inode.find(name)?;
        return Ok(Arc::new_cyclic(|self_ref| MountFSInode {
            inner_inode,
            mount_fs: self.mount_fs.clone(),
            self_ref: self_ref.clone(),
        })
        .overlaid_inode());
    }

    pub(super) fn do_parent(&self) -> Result<Arc<MountFSInode>, SystemError> {
        if self.is_mountpoint_root()? {
            // 当前inode是它所在的文件系统的root inode
            match &self.mount_fs.self_mountpoint {
                Some(inode) => {
                    let inner_inode = inode.parent()?;
                    return Ok(Arc::new_cyclic(|self_ref| MountFSInode {
                        inner_inode,
                        mount_fs: self.mount_fs.clone(),
                        self_ref: self_ref.clone(),
                    }));
                }
                None => {
                    return Ok(self.self_ref.upgrade().unwrap());
                }
            }
        } else {
            let inner_inode = self.inner_inode.parent()?;
            // 向上查找时，不会跨过文件系统的边界，因此直接调用当前inode所在的文件系统的find方法进行查找
            return Ok(Arc::new_cyclic(|self_ref| MountFSInode {
                inner_inode,
                mount_fs: self.mount_fs.clone(),
                self_ref: self_ref.clone(),
            }));
        }
    }

    /// 移除挂载点下的文件系统
    fn do_umount(&self) -> Result<Arc<MountFS>, SystemError> {
        if self.metadata()?.file_type != FileType::Dir {
            return Err(SystemError::ENOTDIR);
        }
        return self
            .mount_fs
            .mountpoints
            .lock()
            .remove(&self.inner_inode.metadata()?.inode_id)
            .ok_or(SystemError::ENOENT);
    }

    fn do_absolute_path(&self) -> Result<String, SystemError> {
        let mut path_parts = Vec::new();
        let mut current = self.self_ref.upgrade().unwrap();
        let mut visited_inodes = alloc::collections::BTreeSet::new();
        const MAX_PATH_DEPTH: usize = 4096; // 防止路径过深

        loop {
            let current_inode_id = current.metadata()?.inode_id;

            // 检查是否到达根节点
            if current_inode_id == ROOT_INODE().metadata()?.inode_id {
                break;
            }

            // 检查循环引用 - 如果已经访问过这个inode，说明存在循环
            if visited_inodes.contains(&current_inode_id) {
                log::error!(
                    "do_absolute_path: detected circular reference at inode_id {:?}",
                    current_inode_id
                );
                return Err(SystemError::ELOOP);
            }

            // 检查路径深度，防止无限循环
            if path_parts.len() >= MAX_PATH_DEPTH {
                log::error!(
                    "do_absolute_path: path depth exceeded {} levels",
                    MAX_PATH_DEPTH
                );
                return Err(SystemError::ENAMETOOLONG);
            }

            visited_inodes.insert(current_inode_id);
            let name = current.dname()?;
            path_parts.push(name.0);

            let parent = current.do_parent()?;
            let parent_inode_id = parent.metadata()?.inode_id;

            // 检查parent是否与current相同，这通常表示出现了问题
            if parent_inode_id == current_inode_id {
                log::warn!("do_absolute_path: parent inode is same as current inode {:?}, treating as namespace root", current_inode_id);
                
                // 遇到namespace边界，尝试从mount list中获取正确路径
                if let Some(namespace) = current.mount_fs.namespace() {
                    let mount_list = namespace.mount_list();
                    
                    // 在mount list中查找当前mount的路径
                    let mount_list_guard = mount_list.0.read();
                    for (mount_path, mount_fs) in mount_list_guard.iter() {
                        if Arc::ptr_eq(mount_fs, &current.mount_fs) {
                            log::info!("do_absolute_path: found mount path in mount list: {}", mount_path.0);
                            
                            // 重新构建完整路径：mount_path + 已收集的子路径
                            path_parts.reverse(); // 先反转，因为我们是从下往上收集的
                            let sub_path = if path_parts.is_empty() {
                                String::new()
                            } else {
                                // 移除最后一个组件（即当前mount的根目录名）
                                if !path_parts.is_empty() {
                                    path_parts.remove(path_parts.len() - 1);
                                }
                                if path_parts.is_empty() {
                                    String::new()
                                } else {
                                    "/".to_string() + &path_parts.iter().map(|s| s.as_str()).collect::<Vec<&str>>().join("/")
                                }
                            };
                            
                            let final_path = if mount_path.0 == "/" {
                                if sub_path.is_empty() {
                                    "/".to_string()
                                } else {
                                    sub_path
                                }
                            } else {
                                mount_path.0.clone() + &sub_path
                            };
                            
                            log::info!("do_absolute_path: constructed final path: {}", final_path);
                            return Ok(final_path);
                        }
                    }
                }
                
                // 如果在mount list中找不到，说明可能是挂载点根
                if current.is_mountpoint_root().unwrap_or(false) {
                    if let Some(mountpoint) = &current.mount_fs.self_mountpoint {
                        log::info!("do_absolute_path: found mountpoint, continuing traversal from parent mount");
                        current = mountpoint.clone();
                        continue;
                    }
                }
                
                // 作为最后手段，直接break
                break;
            }

            current = parent;
        }

        // 由于我们从叶子节点向上遍历到根节点，所以需要反转路径部分
        path_parts.reverse();

        // 构建最终的绝对路径字符串
        let absolute_path = if path_parts.is_empty() {
            // 如果没有路径组件，说明这是根目录
            String::from("/")
        } else {
            // 计算容量：所有部分的长度 + 分隔符数量 + 1（开头的/）
            let mut result = String::with_capacity(
                path_parts.iter().map(|s| s.len()).sum::<usize>() + path_parts.len() + 1,
            );
            for part in path_parts {
                result.push('/');
                result.push_str(&part);
            }
            result
        };

        Ok(absolute_path)
    }
}

impl IndexNode for MountFSInode {
    fn open(
        &self,
        data: SpinLockGuard<FilePrivateData>,
        mode: &FileMode,
    ) -> Result<(), SystemError> {
        return self.inner_inode.open(data, mode);
    }

    fn close(&self, data: SpinLockGuard<FilePrivateData>) -> Result<(), SystemError> {
        return self.inner_inode.close(data);
    }

    fn create_with_data(
        &self,
        name: &str,
        file_type: FileType,
        mode: ModeType,
        data: usize,
    ) -> Result<Arc<dyn IndexNode>, SystemError> {
        let inner_inode = self
            .inner_inode
            .create_with_data(name, file_type, mode, data)?;
        return Ok(Arc::new_cyclic(|self_ref| MountFSInode {
            inner_inode,
            mount_fs: self.mount_fs.clone(),
            self_ref: self_ref.clone(),
        }));
    }

    fn truncate(&self, len: usize) -> Result<(), SystemError> {
        return self.inner_inode.truncate(len);
    }

    fn read_at(
        &self,
        offset: usize,
        len: usize,
        buf: &mut [u8],
        data: SpinLockGuard<FilePrivateData>,
    ) -> Result<usize, SystemError> {
        return self.inner_inode.read_at(offset, len, buf, data);
    }

    fn write_at(
        &self,
        offset: usize,
        len: usize,
        buf: &[u8],
        data: SpinLockGuard<FilePrivateData>,
    ) -> Result<usize, SystemError> {
        return self.inner_inode.write_at(offset, len, buf, data);
    }

    fn read_direct(
        &self,
        offset: usize,
        len: usize,
        buf: &mut [u8],
        data: SpinLockGuard<FilePrivateData>,
    ) -> Result<usize, SystemError> {
        self.inner_inode.read_direct(offset, len, buf, data)
    }

    fn write_direct(
        &self,
        offset: usize,
        len: usize,
        buf: &[u8],
        data: SpinLockGuard<FilePrivateData>,
    ) -> Result<usize, SystemError> {
        self.inner_inode.write_direct(offset, len, buf, data)
    }

    #[inline]
    fn fs(&self) -> Arc<dyn FileSystem> {
        return self.mount_fs.clone();
    }

    #[inline]
    fn as_any_ref(&self) -> &dyn core::any::Any {
        return self.inner_inode.as_any_ref();
    }

    #[inline]
    fn metadata(&self) -> Result<super::Metadata, SystemError> {
        return self.inner_inode.metadata();
    }

    #[inline]
    fn set_metadata(&self, metadata: &super::Metadata) -> Result<(), SystemError> {
        return self.inner_inode.set_metadata(metadata);
    }

    #[inline]
    fn resize(&self, len: usize) -> Result<(), SystemError> {
        return self.inner_inode.resize(len);
    }

    #[inline]
    fn create(
        &self,
        name: &str,
        file_type: FileType,
        mode: ModeType,
    ) -> Result<Arc<dyn IndexNode>, SystemError> {
        let inner_inode = self.inner_inode.create(name, file_type, mode)?;
        return Ok(Arc::new_cyclic(|self_ref| MountFSInode {
            inner_inode,
            mount_fs: self.mount_fs.clone(),
            self_ref: self_ref.clone(),
        }));
    }

    fn link(&self, name: &str, other: &Arc<dyn IndexNode>) -> Result<(), SystemError> {
        return self.inner_inode.link(name, other);
    }

    /// @brief 在挂载文件系统中删除文件/文件夹
    #[inline]
    fn unlink(&self, name: &str) -> Result<(), SystemError> {
        let inode_id = self.inner_inode.find(name)?.metadata()?.inode_id;

        // 先检查这个inode是否为一个挂载点，如果当前inode是一个挂载点，那么就不能删除这个inode
        if self.mount_fs.mountpoints.lock().contains_key(&inode_id) {
            return Err(SystemError::EBUSY);
        }
        // 调用内层的inode的方法来删除这个inode
        return self.inner_inode.unlink(name);
    }

    #[inline]
    fn rmdir(&self, name: &str) -> Result<(), SystemError> {
        let inode_id = self.inner_inode.find(name)?.metadata()?.inode_id;

        // 先检查这个inode是否为一个挂载点，如果当前inode是一个挂载点，那么就不能删除这个inode
        if self.mount_fs.mountpoints.lock().contains_key(&inode_id) {
            return Err(SystemError::EBUSY);
        }
        // 调用内层的rmdir的方法来删除这个inode
        let r = self.inner_inode.rmdir(name);

        return r;
    }

    #[inline]
    fn move_to(
        &self,
        old_name: &str,
        target: &Arc<dyn IndexNode>,
        new_name: &str,
    ) -> Result<(), SystemError> {
        return self.inner_inode.move_to(old_name, target, new_name);
    }

    fn find(&self, name: &str) -> Result<Arc<dyn IndexNode>, SystemError> {
        match name {
            // 查找的是当前目录
            "" | "." => self
                .self_ref
                .upgrade()
                .map(|inode| inode as Arc<dyn IndexNode>)
                .ok_or(SystemError::ENOENT),
            // 往父级查找
            ".." => self.parent(),
            // 在当前目录下查找
            // 直接调用当前inode所在的文件系统的find方法进行查找
            // 由于向下查找可能会跨越文件系统的边界，因此需要尝试替换inode
            _ => self.do_find(name).map(|inode| inode as Arc<dyn IndexNode>),
        }
    }

    #[inline]
    fn get_entry_name(&self, ino: InodeId) -> Result<alloc::string::String, SystemError> {
        return self.inner_inode.get_entry_name(ino);
    }

    #[inline]
    fn get_entry_name_and_metadata(
        &self,
        ino: InodeId,
    ) -> Result<(alloc::string::String, super::Metadata), SystemError> {
        return self.inner_inode.get_entry_name_and_metadata(ino);
    }

    #[inline]
    fn ioctl(
        &self,
        cmd: u32,
        data: usize,
        private_data: &FilePrivateData,
    ) -> Result<usize, SystemError> {
        return self.inner_inode.ioctl(cmd, data, private_data);
    }

    #[inline]
    fn list(&self) -> Result<alloc::vec::Vec<alloc::string::String>, SystemError> {
        return self.inner_inode.list();
    }

    fn mount(&self, fs: Arc<dyn FileSystem>) -> Result<Arc<MountFS>, SystemError> {
        let metadata = self.inner_inode.metadata()?;
        if metadata.file_type != FileType::Dir {
            return Err(SystemError::ENOTDIR);
        }

        if self.is_mountpoint_root()? {
            return Err(SystemError::EBUSY);
        }

        // 若已有挂载系统，保证MountFS只包一层
        let to_mount_fs = fs
            .clone()
            .downcast_arc::<MountFS>()
            .map(|it| it.inner_filesystem())
            .unwrap_or(fs);

        // 获取当前进程的mount namespace并创建namespace感知的MountFS
        let new_mount_fs = {
            use crate::process::namespace::mount_namespace::MountPropagation;
            use crate::process::ProcessManager;

            if ProcessManager::initialized() {
                // 进程管理已初始化，使用当前进程的mount namespace
                let current_pcb = ProcessManager::current_pcb();
                let mount_ns = current_pcb.nsproxy().mount_ns.clone();
                let mount_id = crate::process::namespace::mount_namespace::alloc_global_mount_id();
                let namespace_ref = Arc::downgrade(&mount_ns);

                log::info!(
                    "Mounting filesystem, target: {:?}, mount_id: {}",
                    self.absolute_path(),
                    mount_id
                );

                MountFS::new_with_namespace(
                    to_mount_fs,
                    Some(self.self_ref.upgrade().unwrap()),
                    namespace_ref,
                    MountPropagation::new_private(),
                    mount_id,
                )
            } else {
                // 进程管理未初始化，但我们仍然要创建namespace-aware的MountFS
                // 使用root mount namespace
                let mount_id = crate::process::namespace::mount_namespace::alloc_global_mount_id();
                let root_mount_ns =
                    crate::process::namespace::mount_namespace::init_mount_namespace();
                let namespace_ref = Arc::downgrade(&root_mount_ns);

                log::info!(
                    "Mounting filesystem (early boot), target: {:?}, mount_id: {}",
                    self.absolute_path(),
                    mount_id
                );

                MountFS::new_with_namespace(
                    to_mount_fs,
                    Some(self.self_ref.upgrade().unwrap()),
                    namespace_ref,
                    MountPropagation::new_private(),
                    mount_id,
                )
            }
        };

        // 关键修复：使用当前进程的namespace的root MountFS，而不是self.mount_fs
        let target_mount_fs = if crate::process::ProcessManager::initialized() {
            let current_pcb = crate::process::ProcessManager::current_pcb();
            let mount_ns = current_pcb.nsproxy().mount_ns.clone();

            log::info!(
                "Mount: current namespace id: {}, self.mount_fs id: {}",
                mount_ns.root_mountfs().mount_id(),
                self.mount_fs.mount_id()
            );

            // 找到当前namespace中对应的MountFS
            // 我们需要从当前namespace的root开始，找到包含这个inode的MountFS
            let target = self.find_namespace_mountfs(&mount_ns)?;
            log::info!("Mount: target_mount_fs id: {}", target.mount_id());
            target
        } else {
            self.mount_fs.clone()
        };

        log::info!(
            "Mount: inserting into mountpoints, inode_id: {:?}, new_mount_fs id: {}",
            metadata.inode_id,
            new_mount_fs.mount_id()
        );

        target_mount_fs
            .mountpoints
            .lock()
            .insert(metadata.inode_id, new_mount_fs.clone());

        let mount_path = self.absolute_path()?;

        // 处理mount propagation
        if let Some(namespace) = target_mount_fs.namespace() {
            namespace.handle_mount_propagation(&target_mount_fs, &mount_path, &new_mount_fs)?;
        }

        // 修复：确保在正确的namespace mount list中记录挂载点
        let mount_list = if let Some(namespace) = target_mount_fs.namespace() {
            namespace.mount_list()
        } else {
            MOUNT_LIST()
        };
        
        log::info!("Mount: recording mount in mount list - path: {}, mount_id: {}", mount_path, new_mount_fs.mount_id());
        mount_list.insert(mount_path.clone(), new_mount_fs.clone());
        return Ok(new_mount_fs);
    }

    fn mount_from(&self, from: Arc<dyn IndexNode>) -> Result<Arc<MountFS>, SystemError> {
        let metadata = self.metadata()?;
        if from.metadata()?.file_type != FileType::Dir || metadata.file_type != FileType::Dir {
            return Err(SystemError::ENOTDIR);
        }
        if self.is_mountpoint_root()? {
            return Err(SystemError::EBUSY);
        }
        // debug!("from {:?}, to {:?}", from, self);
        let new_mount_fs = from.umount()?;
        self.mount_fs
            .mountpoints
            .lock()
            .insert(metadata.inode_id, new_mount_fs.clone());

        // MOUNT_LIST().remove(from.absolute_path()?);
        // MOUNT_LIST().insert(self.absolute_path()?, new_mount_fs.clone());
        return Ok(new_mount_fs);
    }

    fn umount(&self) -> Result<Arc<MountFS>, SystemError> {
        if !self.is_mountpoint_root()? {
            return Err(SystemError::EINVAL);
        }

        let mount_path = self.absolute_path()?;

        // 处理umount propagation
        if let Some(namespace) = self.mount_fs.namespace() {
            namespace.handle_umount_propagation(&self.mount_fs, &mount_path)?;
        }

        // 清理propagation状态
        Self::cleanup_mount_propagation_state_for_mount(&self.mount_fs)?;

        return self.mount_fs.umount();
    }



    fn absolute_path(&self) -> Result<String, SystemError> {
        self.do_absolute_path()
    }

    #[inline]
    fn mknod(
        &self,
        filename: &str,
        mode: ModeType,
        dev_t: DeviceNumber,
    ) -> Result<Arc<dyn IndexNode>, SystemError> {
        let inner_inode = self.inner_inode.mknod(filename, mode, dev_t)?;
        return Ok(Arc::new_cyclic(|self_ref| MountFSInode {
            inner_inode,
            mount_fs: self.mount_fs.clone(),
            self_ref: self_ref.clone(),
        }));
    }

    #[inline]
    fn special_node(&self) -> Option<super::SpecialNodeData> {
        self.inner_inode.special_node()
    }

    /// 若不支持，则调用第二种情况来从父目录获取文件名
    /// # Performance
    /// 应尽可能引入DName，
    /// 在默认情况下，性能非常差！！！
    fn dname(&self) -> Result<DName, SystemError> {
        if self.is_mountpoint_root()? {
            if let Some(inode) = &self.mount_fs.self_mountpoint {
                return inode.inner_inode.dname();
            }
        }
        return self.inner_inode.dname();
    }

    fn parent(&self) -> Result<Arc<dyn IndexNode>, SystemError> {
        return self.do_parent().map(|inode| inode as Arc<dyn IndexNode>);
    }

    fn page_cache(&self) -> Option<Arc<PageCache>> {
        self.inner_inode.page_cache()
    }

    fn as_pollable_inode(&self) -> Result<&dyn PollableInode, SystemError> {
        self.inner_inode.as_pollable_inode()
    }
}

impl FileSystem for MountFS {
    fn root_inode(&self) -> Arc<dyn IndexNode> {
        match &self.self_mountpoint {
            Some(inode) => return inode.mount_fs.root_inode(),
            // 当前文件系统是rootfs
            None => self.mountpoint_root_inode(),
        }
    }

    fn info(&self) -> super::FsInfo {
        return self.inner_filesystem.info();
    }

    /// @brief 本函数用于实现动态转换。
    /// 具体的文件系统在实现本函数时，最简单的方式就是：直接返回self
    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "mountfs"
    }
    fn super_block(&self) -> SuperBlock {
        SuperBlock::new(Magic::MOUNT_MAGIC, MOUNTFS_BLOCK_SIZE, MOUNTFS_MAX_NAMELEN)
    }

    unsafe fn fault(&self, pfm: &mut PageFaultMessage) -> VmFaultReason {
        self.inner_filesystem.fault(pfm)
    }

    unsafe fn map_pages(
        &self,
        pfm: &mut PageFaultMessage,
        start_pgoff: usize,
        end_pgoff: usize,
    ) -> VmFaultReason {
        self.inner_filesystem.map_pages(pfm, start_pgoff, end_pgoff)
    }
}

/// MountList
/// ```rust
/// use alloc::collection::BTreeSet;
/// let map = BTreeSet::from([
///     "/sys", "/dev", "/", "/bin", "/proc"
/// ]);
/// assert_eq!(format!("{:?}", map), "{\"/\", \"/bin\", \"/dev\", \"/proc\", \"/sys\"}");
/// // {"/", "/bin", "/dev", "/proc", "/sys"}
/// ```
#[derive(PartialEq, Eq, Debug, Clone)]
pub struct MountPath(String);

impl From<&str> for MountPath {
    fn from(value: &str) -> Self {
        Self(String::from(value))
    }
}

impl From<String> for MountPath {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl AsRef<str> for MountPath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Display for MountPath {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl PartialOrd for MountPath {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MountPath {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        let self_dep = self.0.chars().filter(|c| *c == '/').count();
        let othe_dep = other.0.chars().filter(|c| *c == '/').count();
        if self_dep == othe_dep {
            // 深度一样时反序来排
            // 根目录和根目录下的文件的绝对路径都只有一个'/'
            other.0.cmp(&self.0)
        } else {
            // 根据深度，深度
            othe_dep.cmp(&self_dep)
        }
    }
}

// 维护一个挂载点的记录，以支持特定于文件系统的索引
pub struct MountList(RwLock<BTreeMap<MountPath, Arc<MountFS>>>);
// pub struct MountList(Option<Arc<MountListInner>>);
static mut __MOUNTS_LIST: Option<Arc<MountList>> = None;

/// # init_mountlist - 初始化挂载列表
///
/// 此函数用于初始化系统的挂载列表。挂载列表记录了系统中所有的文件系统挂载点及其属性。
///
/// ## 参数
///
/// - 无
///
/// ## 返回值
///
/// - 无
#[inline(always)]
pub fn init_mountlist() {
    unsafe {
        __MOUNTS_LIST = Some(Arc::new(MountList(RwLock::new(BTreeMap::new()))));
    }
}

/// # MOUNT_LIST - 获取挂载列表
///
/// 该函数用于获取挂载列表引用。在进程管理初始化之前，返回全局挂载列表；
/// 初始化之后，返回当前进程mount namespace的挂载列表。
///
/// ## 返回值
/// - Arc<MountList>: 返回挂载列表的引用。
#[inline(always)]
#[allow(non_snake_case)]
pub fn MOUNT_LIST() -> Arc<MountList> {
    use crate::process::ProcessManager;

    // 检查进程管理是否已经初始化
    if !ProcessManager::initialized() {
        // 进程管理未初始化，使用全局挂载列表
        return GLOBAL_MOUNT_LIST().clone();
    }

    // 进程管理已初始化，获取当前进程的mount namespace
    let current_pcb = ProcessManager::current_pcb();
    let mount_ns = current_pcb.nsproxy().mount_ns.clone();
    mount_ns.mount_list()
}

/// # GLOBAL_MOUNT_LIST - 获取全局挂载列表（为了向后兼容）
///
/// 该函数用于获取全局挂载列表的引用。主要用于系统初始化和兼容性目的。
///
/// ## 返回值
/// - &'static Arc<MountList>: 返回全局挂载列表的引用。
#[inline(always)]
#[allow(non_snake_case)]
pub fn GLOBAL_MOUNT_LIST() -> &'static Arc<MountList> {
    unsafe {
        return __MOUNTS_LIST.as_ref().unwrap();
    }
}

impl MountList {
    /// 创建一个新的空挂载列表
    pub fn new_empty() -> Self {
        use crate::libs::rwlock::RwLock;
        use alloc::collections::BTreeMap;

        Self(RwLock::new(BTreeMap::new()))
    }

    /// 复制挂载列表内容到另一个挂载列表
    /// 用于namespace复制
    pub fn copy_to(
        &self,
        target: &MountList,
        new_namespace: Weak<crate::process::namespace::mount_namespace::MountNamespace>,
    ) -> Result<(), SystemError> {
        let source_map = self.0.read();
        let mut target_map = target.0.write();

        // 为每个挂载点创建新的MountFS实例，确保隔离
        for (path, mount_fs) in source_map.iter() {
            let new_mount_fs = mount_fs.deep_copy_for_namespace(
                new_namespace.clone(),
                mount_fs.mount_id(), // 保持相同的mount_id用于识别
            );
            target_map.insert(path.clone(), new_mount_fs);
        }

        Ok(())
    }

    /// # insert - 将文件系统挂载点插入到挂载表中
    ///
    /// 将一个新的文件系统挂载点插入到挂载表中。如果挂载点已经存在，则会更新对应的文件系统。
    ///
    /// 此函数是线程安全的，因为它使用了RwLock来保证并发访问。
    ///
    /// ## 参数
    ///
    /// - `path`: &str, 挂载点的路径。这个路径会被转换成`MountPath`类型。
    /// - `fs`: Arc<MountFS>, 共享的文件系统实例。
    ///
    /// ## 返回值
    ///
    /// - 无
    #[inline]
    pub fn insert<T: AsRef<str>>(&self, path: T, fs: Arc<MountFS>) {
        self.0.write().insert(MountPath::from(path.as_ref()), fs);
    }

    /// # get_mount_point - 获取挂载点的路径
    ///
    /// 这个函数用于查找给定路径的挂载点。它搜索一个内部映射，找到与路径匹配的挂载点。
    ///
    /// ## 参数
    ///
    /// - `path: T`: 这是一个可转换为字符串的引用，表示要查找其挂载点的路径。
    ///
    /// ## 返回值
    ///
    /// - `Option<(String, String, Arc<MountFS>)>`:
    ///   - `Some((mount_point, rest_path, fs))`: 如果找到了匹配的挂载点，返回一个包含挂载点路径、剩余路径和挂载文件系统的元组。
    ///   - `None`: 如果没有找到匹配的挂载点，返回 None。
    #[inline]
    #[allow(dead_code)]
    pub fn get_mount_point<T: AsRef<str>>(
        &self,
        path: T,
    ) -> Option<(String, String, Arc<MountFS>)> {
        self.0
            .upgradeable_read()
            .iter()
            .filter_map(|(key, fs)| {
                let strkey = key.as_ref();
                if let Some(rest) = path.as_ref().strip_prefix(strkey) {
                    return Some((strkey.to_string(), rest.to_string(), fs.clone()));
                }
                None
            })
            .next()
    }

    /// # remove - 移除挂载点
    ///
    /// 从挂载点管理器中移除一个挂载点。
    ///
    /// 此函数用于从挂载点管理器中移除一个已经存在的挂载点。如果挂载点不存在，则不进行任何操作。
    ///
    /// ## 参数
    ///
    /// - `path: T`: `T` 实现了 `Into<MountPath>`  trait，代表要移除的挂载点的路径。
    ///
    /// ## 返回值
    ///
    /// - `Option<Arc<MountFS>>`: 返回一个 `Arc<MountFS>` 类型的可选值，表示被移除的挂载点，如果挂载点不存在则返回 `None`。
    #[inline]
    pub fn remove<T: Into<MountPath>>(&self, path: T) -> Option<Arc<MountFS>> {
        self.0.write().remove(&path.into())
    }
}

impl Debug for MountList {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_map().entries(self.0.read().iter()).finish()
    }
}

/// 判断给定的inode是否为其所在文件系统的根inode
///
/// ## 返回值
///
/// - `true`: 是根inode
/// - `false`: 不是根inode或者传入的inode不是MountFSInode类型，或者调用inode的metadata方法时报错了。
pub fn is_mountpoint_root(inode: &Arc<dyn IndexNode>) -> bool {
    let mnt_inode = inode.as_any_ref().downcast_ref::<MountFSInode>();
    if let Some(mnt) = mnt_inode {
        return mnt.is_mountpoint_root().unwrap_or(false);
    }

    return false;
}

/// # do_mount_mkdir - 在指定挂载点创建目录并挂载文件系统
///
/// 在指定的挂载点创建一个目录，并将其挂载到文件系统中。如果挂载点已经存在，并且不是空的，
/// 则会返回错误。成功时，会返回一个新的挂载文件系统的引用。
///
/// ## 参数
///
/// - `fs`: FileSystem - 文件系统的引用，用于创建和挂载目录。
/// - `mount_point`: &str - 挂载点路径，用于创建和挂载目录。
///
/// ## 返回值
///
/// - `Ok(Arc<MountFS>)`: 成功挂载文件系统后，返回挂载文件系统的共享引用。
/// - `Err(SystemError)`: 挂载失败时，返回系统错误。
pub fn do_mount_mkdir(
    fs: Arc<dyn FileSystem>,
    mount_point: &str,
) -> Result<Arc<MountFS>, SystemError> {
    let inode = do_mkdir_at(
        AtFlags::AT_FDCWD.bits(),
        mount_point,
        FileMode::from_bits_truncate(0o755),
    )?;
    if let Some((_, rest, _fs)) = MOUNT_LIST().get_mount_point(mount_point) {
        if rest.is_empty() {
            return Err(SystemError::EBUSY);
        }
    }
    return inode.mount(fs);
}
