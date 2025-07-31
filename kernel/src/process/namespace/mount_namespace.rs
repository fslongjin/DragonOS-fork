use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};
use hashbrown::HashMap;
use system_error::SystemError;

use crate::{
    filesystem::vfs::{mount::MountFS, mount::MountList},
    libs::spinlock::SpinLock,
    process::fork::CloneFlags,
};

use core::sync::atomic::{AtomicU32, Ordering};

use super::{
    nsproxy::NsCommon, user_namespace::UserNamespace, NamespaceOps, NamespaceType,
};

/// 全局mount ID分配器 - 所有namespace共享
static GLOBAL_MOUNT_ID: AtomicU32 = AtomicU32::new(1);

/// 分配全局唯一的mount ID
pub fn alloc_global_mount_id() -> u32 {
    GLOBAL_MOUNT_ID.fetch_add(1, Ordering::SeqCst)
}

/// Mount namespace结构 - 管理挂载树和propagation
pub struct MountNamespace {
    ns_common: NsCommon,
    self_ref: Weak<MountNamespace>,
    parent: Option<Weak<MountNamespace>>,
    user_ns: Arc<UserNamespace>,
    
    inner: SpinLock<InnerMountNamespace>,
}

struct InnerMountNamespace {
    /// 当前namespace的根MountFS（复用现有实现）
    root_mountfs: Arc<MountFS>,
    /// namespace特有的挂载列表
    mount_list: Arc<MountList>,
    /// propagation组管理
    shared_groups: HashMap<u32, SharedGroup>,
    dead: bool,
}

/// 共享组，用于管理shared propagation
struct SharedGroup {
    group_id: u32,
    members: Vec<Weak<MountFS>>,
}

/// Propagation元数据 - 扩展到MountFS
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PropagationType {
    Private,
    Shared,
    Slave, 
    Unbindable,
}

/// Mount propagation信息
#[derive(Debug)]
pub struct MountPropagation {
    pub prop_type: PropagationType,
    pub shared_group_id: Option<u32>,
    pub master: Option<Weak<MountFS>>,
}

impl MountPropagation {
    pub fn new_private() -> Self {
        Self {
            prop_type: PropagationType::Private,
            shared_group_id: None,
            master: None,
        }
    }
    
    pub fn new_shared(group_id: u32) -> Self {
        Self {
            prop_type: PropagationType::Shared,
            shared_group_id: Some(group_id),
            master: None,
        }
    }
}

impl MountNamespace {
    /// 创建root mount namespace（延迟初始化版本）
    pub fn new_root() -> Arc<Self> {
        use crate::filesystem::vfs::mount::init_mountlist;
        
        // 首先确保全局挂载列表已初始化
        init_mountlist();
        
        // 创建namespace专用的挂载列表
        let mount_list = Arc::new(MountList::new_empty());
        
        // 创建一个简单的ramfs作为占位符，避免循环依赖
        let placeholder_fs = crate::filesystem::ramfs::RamFS::new();
        let placeholder_mountfs = crate::filesystem::vfs::mount::MountFS::new_with_namespace(
            placeholder_fs,
            None,
            Weak::new(),
            MountPropagation::new_private(),
            0,
        );
        
        Arc::new_cyclic(|self_ref| {
            Self {
                ns_common: NsCommon::new(0, NamespaceType::Mount),
                self_ref: self_ref.clone(),
                parent: None,
                user_ns: super::user_namespace::INIT_USER_NAMESPACE.clone(),
                inner: SpinLock::new(InnerMountNamespace {
                    root_mountfs: placeholder_mountfs,
                    mount_list,
                    shared_groups: HashMap::new(),
                    dead: false,
                }),
            }
        })
    }
    
    /// 设置根MountFS（在vfs初始化后调用）
    pub fn set_root_mountfs(&self, root_mountfs: Arc<crate::filesystem::vfs::mount::MountFS>) {
        // 为新的MountFS设置namespace引用
        root_mountfs.set_namespace(self.self_ref.clone());
        
        let mut inner = self.inner.lock();
        inner.root_mountfs = root_mountfs.clone();
    }
    
    /// 复制mount namespace（用于CLONE_NEWNS）
    pub fn copy_mount_ns(
        &self,
        clone_flags: &CloneFlags,
        user_ns: Arc<UserNamespace>,
    ) -> Result<Arc<Self>, SystemError> {
        if !clone_flags.contains(CloneFlags::CLONE_NEWNS) {
            return Ok(self.self_ref.upgrade().unwrap());
        }
        
        self.create_mount_namespace(user_ns)
    }
    
    /// 创建新的mount namespace
    pub fn create_mount_namespace(
        &self,
        user_ns: Arc<UserNamespace>,
    ) -> Result<Arc<Self>, SystemError> {
        let inner = self.inner.lock();
        let new_mount_list = Arc::new(MountList::new_empty());
        
        // 复制挂载列表内容
        self.copy_mount_list(&inner.mount_list, &new_mount_list)?;
        
        let new_ns = Arc::new_cyclic(|self_ref| {
            // 使用深度复制方法创建新的root_mountfs，分配新的全局mount ID
            let new_root_mountfs = inner.root_mountfs.deep_copy_for_namespace(
                self_ref.clone(),
                alloc_global_mount_id(), // 为复制的root分配新的全局mount ID
            );
            
            log::info!("Created new mount namespace, root mount_id: {}", new_root_mountfs.mount_id());
            
            Self {
                ns_common: NsCommon::new(self.ns_common.level + 1, NamespaceType::Mount),
                self_ref: self_ref.clone(),
                parent: Some(self.self_ref.clone()),
                user_ns,
                inner: SpinLock::new(InnerMountNamespace {
                    root_mountfs: new_root_mountfs,
                    mount_list: new_mount_list,
                    shared_groups: HashMap::new(), // 新namespace从空的共享组开始
                    dead: false,
                }),
            }
        });
        
        Ok(new_ns)
    }
    
    /// 获取namespace感知的挂载列表
    pub fn mount_list(&self) -> Arc<MountList> {
        self.inner.lock().mount_list.clone()
    }
    
    /// 获取当前namespace的根MountFS
    pub fn root_mountfs(&self) -> Arc<MountFS> {
        self.inner.lock().root_mountfs.clone()
    }
    

    
    /// 创建或加入共享组
    pub fn create_or_join_shared_group(&self, mount: Weak<MountFS>) -> Result<u32, SystemError> {
        let mut inner = self.inner.lock();
        
        // 找到一个可用的组ID
        let group_id = inner.shared_groups.len() as u32 + 1;
        
        let group = SharedGroup {
            group_id,
            members: vec![mount],
        };
        
        inner.shared_groups.insert(group_id, group);
        Ok(group_id)
    }
    
    /// 从共享组中移除成员
    pub fn leave_shared_group(&self, group_id: u32, mount: &Weak<MountFS>) -> Result<(), SystemError> {
        let mut inner = self.inner.lock();
        
        if let Some(group) = inner.shared_groups.get_mut(&group_id) {
            group.members.retain(|m| !Weak::ptr_eq(m, mount));
            
            // 如果组为空，删除这个组
            if group.members.is_empty() {
                inner.shared_groups.remove(&group_id);
            }
        }
        
        Ok(())
    }
    
    /// 复制挂载列表内容
    fn copy_mount_list(&self, source: &Arc<MountList>, target: &Arc<MountList>) -> Result<(), SystemError> {
        // 使用MountList的copy_to方法进行复制
        source.copy_to(target, self.self_ref.clone())?;
        Ok(())
    }
    
    /// 处理挂载传播
    pub fn handle_mount_propagation(
        &self,
        source_mount: &Arc<MountFS>,
        _target_path: &str,
        new_mount: &Arc<MountFS>,
    ) -> Result<(), SystemError> {
        let prop_type = source_mount.propagation();
        
        match prop_type {
            PropagationType::Shared => {
                // 传播到同一共享组的所有挂载点
                // 这里需要获取shared_group_id，先简化处理
                // TODO: 从MountFS中获取shared_group_id
                // self.propagate_to_shared_group(group_id, target_path, new_mount)?;
            },
            PropagationType::Slave => {
                // 从属挂载从其主挂载接收传播，但不向外传播
                // 这里不需要额外操作，因为slave挂载不会向外传播
            },
            PropagationType::Private => {
                // 私有挂载不传播
            },
            PropagationType::Unbindable => {
                // 不可绑定的挂载不允许绑定挂载
                if source_mount.as_ref() as *const _ == new_mount.as_ref() as *const _ {
                    return Err(SystemError::EINVAL);
                }
            },
        }
        
        Ok(())
    }
    
    /// 传播到共享组
    fn propagate_to_shared_group(
        &self,
        group_id: u32,
        _target_path: &str,
        new_mount: &Arc<MountFS>,
    ) -> Result<(), SystemError> {
        let inner = self.inner.lock();
        if let Some(group) = inner.shared_groups.get(&group_id) {
            for member_weak in &group.members {
                if let Some(member) = member_weak.upgrade() {
                    // 跳过源挂载点，避免重复传播
                    if Arc::ptr_eq(&member, new_mount) {
                        continue;
                    }
                    
                    // 在每个成员上创建相应的挂载
                    // 这是简化的实现，实际中需要更复杂的路径解析和挂载逻辑
                    log::debug!("Propagating mount to shared group member at path: {}", _target_path);
                    
                    // 实际实现中，这里应该：
                    // 1. 解析target_path在member的namespace中的对应路径
                    // 2. 在该路径上创建新的挂载
                    // 3. 复制new_mount的文件系统
                    
                    // 为了简化，这里只记录传播事件
                }
            }
        }
        Ok(())
    }
    
    /// 处理卸载传播
    pub fn handle_umount_propagation(
        &self,
        source_mount: &Arc<MountFS>,
        _target_path: &str,
    ) -> Result<(), SystemError> {
        let prop_type = source_mount.propagation();
        
        match prop_type {
            PropagationType::Shared => {
                // 传播卸载到同一共享组的所有挂载点
                // 这里需要获取shared_group_id，先简化处理
                // TODO: 从MountFS中获取shared_group_id
                // self.propagate_umount_to_shared_group(group_id, target_path, source_mount)?;
            },
            PropagationType::Slave => {
                // 从属挂载不向外传播卸载
            },
            PropagationType::Private => {
                // 私有挂载不传播
            },
            PropagationType::Unbindable => {
                // 不可绑定挂载的卸载不传播
            },
        }
        
        Ok(())
    }
    
    /// 传播卸载到共享组
    fn propagate_umount_to_shared_group(
        &self,
        group_id: u32,
        _target_path: &str,
        source_mount: &Arc<MountFS>,
    ) -> Result<(), SystemError> {
        let inner = self.inner.lock();
        if let Some(group) = inner.shared_groups.get(&group_id) {
            for member_weak in &group.members {
                if let Some(member) = member_weak.upgrade() {
                    // 跳过源挂载点
                    if Arc::ptr_eq(&member, source_mount) {
                        continue;
                    }
                    
                    log::debug!("Propagating umount to shared group member at path: {}", _target_path);
                    
                    // 实际实现中，这里应该在对应的挂载点执行卸载操作
                }
            }
        }
        Ok(())
    }
}

impl NamespaceOps for MountNamespace {
    fn ns_common(&self) -> &NsCommon {
        &self.ns_common
    }
}

impl core::fmt::Debug for MountNamespace {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MountNamespace")
            .field("level", &self.ns_common.level)
            .field("type", &self.ns_common.ty())
            .finish()
    }
}

/// 初始化root mount namespace的全局实例
static mut INIT_MOUNT_NAMESPACE: Option<Arc<MountNamespace>> = None;

/// 获取初始mount namespace的引用
pub fn init_mount_namespace() -> Arc<MountNamespace> {
    unsafe {
        if INIT_MOUNT_NAMESPACE.is_none() {
            INIT_MOUNT_NAMESPACE = Some(MountNamespace::new_root());
        }
        INIT_MOUNT_NAMESPACE.as_ref().unwrap().clone()
    }
}

