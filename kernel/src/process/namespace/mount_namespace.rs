use alloc::{
    string::String,
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

use super::{nsproxy::NsCommon, user_namespace::UserNamespace, NamespaceOps, NamespaceType};

/// 全局mount ID分配器 - 所有namespace共享
static GLOBAL_MOUNT_ID: AtomicU32 = AtomicU32::new(1);

/// 全局shared group ID分配器 - 所有namespace共享，支持跨namespace的peer group
static GLOBAL_SHARED_GROUP_ID: AtomicU32 = AtomicU32::new(1);

/// 分配全局唯一的mount ID
pub fn alloc_global_mount_id() -> u32 {
    GLOBAL_MOUNT_ID.fetch_add(1, Ordering::SeqCst)
}

/// 分配全局唯一的shared group ID（类似Linux的mnt_group_ida）
pub fn alloc_global_shared_group_id() -> u32 {
    GLOBAL_SHARED_GROUP_ID.fetch_add(1, Ordering::SeqCst)
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

impl SharedGroup {
    fn new(group_id: u32) -> Self {
        Self {
            group_id,
            members: Vec::new(),
        }
    }

    fn add_member(&mut self, mount: Weak<MountFS>) {
        self.members.push(mount);
    }

    fn remove_member(&mut self, mount: &Weak<MountFS>) {
        self.members.retain(|m| !Weak::ptr_eq(m, mount));
    }

    fn cleanup_stale_members(&mut self) {
        self.members.retain(|m| m.upgrade().is_some());
    }

    fn id(&self) -> u32 {
        self.group_id
    }

    fn member_count(&self) -> usize {
        self.members.len()
    }
}

/// Propagation元数据 - 扩展到MountFS
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropagationType {
    Private,
    Shared,
    Slave,
    Unbindable,
}

/// 传播标志位
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropagationFlags {
    bits: u32,
}

impl PropagationFlags {
    const RECURSIVE: u32 = 1 << 0; // 递归传播（MS_REC）
    const LOCKED: u32 = 1 << 1; // 传播锁定状态
    const PENDING: u32 = 1 << 2; // 有pending的传播事件
    const PROPAGATING: u32 = 1 << 3; // 正在传播中

    pub const fn empty() -> Self {
        Self { bits: 0 }
    }

    pub const fn from_bits(bits: u32) -> Self {
        Self { bits }
    }

    pub const fn recursive() -> Self {
        Self {
            bits: Self::RECURSIVE,
        }
    }

    pub const fn locked() -> Self {
        Self { bits: Self::LOCKED }
    }

    pub const fn pending() -> Self {
        Self {
            bits: Self::PENDING,
        }
    }

    pub const fn propagating() -> Self {
        Self {
            bits: Self::PROPAGATING,
        }
    }

    pub const fn contains(&self, other: Self) -> bool {
        (self.bits & other.bits) == other.bits
    }

    pub fn insert(&mut self, other: Self) {
        self.bits |= other.bits;
    }

    pub fn remove(&mut self, other: Self) {
        self.bits &= !other.bits;
    }

    pub const fn is_empty(&self) -> bool {
        self.bits == 0
    }
}

/// Mount propagation信息 - 增强版本
#[derive(Debug, Clone)]
pub struct MountPropagation {
    pub prop_type: PropagationType,
    pub shared_group_id: Option<u32>,
    pub master: Option<Weak<MountFS>>, // slave挂载的master引用
    pub slaves: Vec<Weak<MountFS>>,    // 从属挂载列表
    pub peer_group_id: Option<u32>,    // 对等组ID，用于更复杂的传播关系
    pub flags: PropagationFlags,       // 传播标志
    pub propagation_count: u32,        // 传播计数器，用于防止循环
}

impl MountPropagation {
    pub fn new_private() -> Self {
        Self {
            prop_type: PropagationType::Private,
            shared_group_id: None,
            master: None,
            slaves: Vec::new(),
            peer_group_id: None,
            flags: PropagationFlags::empty(),
            propagation_count: 0,
        }
    }

    pub fn new_shared(group_id: u32) -> Self {
        Self {
            prop_type: PropagationType::Shared,
            shared_group_id: Some(group_id),
            master: None,
            slaves: Vec::new(),
            peer_group_id: Some(group_id),
            flags: PropagationFlags::empty(),
            propagation_count: 0,
        }
    }

    pub fn new_slave(master: Weak<MountFS>) -> Self {
        Self {
            prop_type: PropagationType::Slave,
            shared_group_id: None,
            master: Some(master),
            slaves: Vec::new(),
            peer_group_id: None,
            flags: PropagationFlags::empty(),
            propagation_count: 0,
        }
    }

    pub fn new_unbindable() -> Self {
        Self {
            prop_type: PropagationType::Unbindable,
            shared_group_id: None,
            master: None,
            slaves: Vec::new(),
            peer_group_id: None,
            flags: PropagationFlags::empty(),
            propagation_count: 0,
        }
    }

    /// 添加一个从属挂载
    pub fn add_slave(&mut self, slave: Weak<MountFS>) {
        self.slaves.push(slave);
    }

    /// 移除一个从属挂载
    pub fn remove_slave(&mut self, slave: &Weak<MountFS>) {
        self.slaves.retain(|s| !Weak::ptr_eq(s, slave));
    }

    /// 清理无效的从属挂载引用
    pub fn cleanup_stale_slaves(&mut self) {
        self.slaves.retain(|s| s.upgrade().is_some());
    }

    /// 检查是否可以传播（防止循环）
    pub fn can_propagate(&self, max_depth: u32) -> bool {
        self.propagation_count < max_depth && !self.flags.contains(PropagationFlags::propagating())
    }

    /// 开始传播（设置传播标志）
    pub fn start_propagation(&mut self) {
        self.flags.insert(PropagationFlags::propagating());
        self.propagation_count += 1;
    }

    /// 结束传播（清除传播标志）
    pub fn end_propagation(&mut self) {
        self.flags.remove(PropagationFlags::propagating());
    }

    /// 重置传播计数器
    pub fn reset_propagation_count(&mut self) {
        self.propagation_count = 0;
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

        Arc::new_cyclic(|self_ref| Self {
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

            log::info!(
                "Created new mount namespace, root mount_id: {}",
                new_root_mountfs.mount_id()
            );

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

    /// 获取共享组信息（用于传播引擎）
    pub fn get_shared_group_members(&self, group_id: u32) -> Vec<Arc<MountFS>> {
        let inner = self.inner.lock();
        if let Some(group) = inner.shared_groups.get(&group_id) {
            group
                .members
                .iter()
                .filter_map(|weak| weak.upgrade())
                .collect()
        } else {
            Vec::new()
        }
    }

    /// 创建新的共享组（使用全局唯一ID）
    pub fn create_shared_group(&self, mount: Weak<MountFS>) -> Result<u32, SystemError> {
        let mut inner = self.inner.lock();

        // 分配全局唯一的shared group ID
        let group_id = alloc_global_shared_group_id();

        let mut group = SharedGroup::new(group_id);
        group.add_member(mount);

        inner.shared_groups.insert(group_id, group);
        log::info!(
            "MountNamespace: created new shared group {} with global ID",
            group_id
        );
        Ok(group_id)
    }

    /// 加入现有的共享组（支持跨namespace）
    pub fn join_shared_group(
        &self,
        group_id: u32,
        mount: Weak<MountFS>,
    ) -> Result<(), SystemError> {
        let mut inner = self.inner.lock();

        if let Some(group) = inner.shared_groups.get_mut(&group_id) {
            group.add_member(mount);
            log::info!(
                "MountNamespace: added member to existing shared group {}, now has {} members",
                group_id,
                group.member_count()
            );
            Ok(())
        } else {
            // 如果本namespace中没有这个group，创建一个代理group
            // 这支持跨namespace的shared group
            let mut group = SharedGroup::new(group_id);
            group.add_member(mount);

            inner.shared_groups.insert(group_id, group);
            log::info!(
                "MountNamespace: created proxy shared group {} for cross-namespace propagation",
                group_id
            );
            Ok(())
        }
    }

    /// 创建或加入共享组（兼容性方法）
    pub fn create_or_join_shared_group(&self, mount: Weak<MountFS>) -> Result<u32, SystemError> {
        // 对于新的shared mount，总是创建新的shared group
        // 这更符合Linux的行为，每个mount --make-shared都创建新的peer group
        self.create_shared_group(mount)
    }

    /// 从共享组中移除成员
    pub fn leave_shared_group(
        &self,
        group_id: u32,
        mount: &Weak<MountFS>,
    ) -> Result<(), SystemError> {
        let mut inner = self.inner.lock();

        if let Some(group) = inner.shared_groups.get_mut(&group_id) {
            group.remove_member(mount);

            // 如果组为空，删除这个组
            if group.member_count() == 0 {
                inner.shared_groups.remove(&group_id);
                log::info!("MountNamespace: removed empty shared group {}", group_id);
            } else {
                log::info!(
                    "MountNamespace: removed member from shared group {}, {} members remaining",
                    group.id(),
                    group.member_count()
                );
            }
        }

        Ok(())
    }

    /// 复制挂载列表内容
    fn copy_mount_list(
        &self,
        source: &Arc<MountList>,
        target: &Arc<MountList>,
    ) -> Result<(), SystemError> {
        // 使用MountList的copy_to方法进行复制
        source.copy_to(target, self.self_ref.clone())?;
        Ok(())
    }

    /// 处理挂载传播 - 使用传播引擎
    pub fn handle_mount_propagation(
        &self,
        source_mount: &Arc<MountFS>,
        target_path: &str,
        new_mount: &Arc<MountFS>,
    ) -> Result<(), SystemError> {
        // 使用传播引擎处理
        let engine = crate::filesystem::vfs::propagation::get_propagation_engine();
        engine.handle_mount_event(source_mount, target_path, new_mount, 0)?;
        Ok(())
    }

    /// 处理传播类型变更（mount --make-shared等）
    pub fn change_propagation_type(
        &self,
        mount: &Arc<MountFS>,
        new_type: PropagationType,
        recursive: bool,
    ) -> Result<(), SystemError> {
        let old_type = mount.propagation();

        log::info!(
            "MountNamespace: changing propagation type {:?} -> {:?}, recursive: {}",
            old_type,
            new_type,
            recursive
        );

        // 实际变更传播类型
        mount.set_propagation(new_type)?;

        // 如果是递归变更，处理所有子挂载
        if recursive {
            self.change_propagation_recursive(mount, new_type)?;
        }

        // 使用传播引擎处理传播类型变更事件
        let engine = crate::filesystem::vfs::propagation::get_propagation_engine();
        engine.handle_propagation_change_event(mount, old_type, new_type, recursive)?;

        Ok(())
    }

    /// 递归变更子挂载的传播类型
    fn change_propagation_recursive(
        &self,
        mount: &Arc<MountFS>,
        new_type: PropagationType,
    ) -> Result<(), SystemError> {
        let child_mounts = mount.get_child_mounts();

        for (_, child_mount) in child_mounts {
            log::info!(
                "MountNamespace: recursively changing propagation for mount_id {}",
                child_mount.mount_id()
            );
            child_mount.set_propagation(new_type)?;
            self.change_propagation_recursive(&child_mount, new_type)?;
        }

        Ok(())
    }

    /// 处理卸载传播 - 使用传播引擎
    pub fn handle_umount_propagation(
        &self,
        source_mount: &Arc<MountFS>,
        target_path: &str,
    ) -> Result<(), SystemError> {
        // 使用传播引擎处理
        let engine = crate::filesystem::vfs::propagation::get_propagation_engine();
        engine.handle_umount_event(source_mount, target_path, 0)?;
        Ok(())
    }

    /// 建立master-slave关系
    pub fn establish_master_slave_relationship(
        &self,
        master: &Arc<MountFS>,
        slave: &Arc<MountFS>,
    ) -> Result<(), SystemError> {
        log::info!(
            "MountNamespace: establishing master-slave relationship, master: {}, slave: {}",
            master.mount_id(),
            slave.mount_id()
        );

        // 在master中添加slave
        master.add_slave_mount(slave.clone())?;

        // 设置slave的master和传播类型
        slave.set_master_mount(Some(master.clone()))?;
        slave.set_propagation(PropagationType::Slave)?;

        // 清除slave的共享组信息
        let prop_info = slave.get_propagation_info();
        if let Some(group_id) = prop_info.shared_group_id {
            self.leave_shared_group(group_id, &Arc::downgrade(slave))?;
            slave.set_shared_group_id(None)?;
        }

        Ok(())
    }

    /// 打破master-slave关系
    pub fn break_master_slave_relationship(
        &self,
        master: &Arc<MountFS>,
        slave: &Arc<MountFS>,
    ) -> Result<(), SystemError> {
        log::info!(
            "MountNamespace: breaking master-slave relationship, master: {}, slave: {}",
            master.mount_id(),
            slave.mount_id()
        );

        // 从master的slaves列表中移除slave
        master.remove_slave_mount(slave)?;

        // 清除slave的master引用和传播类型
        slave.set_master_mount(None)?;
        slave.set_propagation(PropagationType::Private)?; // 默认变为private

        Ok(())
    }

    /// 处理bind mount
    pub fn handle_bind_mount(
        &self,
        source_mount: &Arc<MountFS>,
        target_path: &str,
        flags: u32,
    ) -> Result<(), SystemError> {
        log::info!(
            "MountNamespace: handling bind mount from mount_id {} to {}",
            source_mount.mount_id(),
            target_path
        );

        // 检查source是否为unbindable
        if source_mount.propagation() == PropagationType::Unbindable {
            log::error!("MountNamespace: bind mount on unbindable filesystem");
            return Err(SystemError::EINVAL);
        }

        // 使用传播引擎处理bind mount
        let engine = crate::filesystem::vfs::propagation::get_propagation_engine();
        engine.handle_bind_mount_event(source_mount, target_path, flags)?;

        Ok(())
    }

    /// 获取挂载传播信息的人类可读格式（用于调试）
    pub fn get_mount_propagation_info(&self, mount: &Arc<MountFS>) -> String {
        let prop_info = mount.get_propagation_info();

        let prop_type_str = match prop_info.prop_type {
            PropagationType::Private => "private",
            PropagationType::Shared => "shared",
            PropagationType::Slave => "slave",
            PropagationType::Unbindable => "unbindable",
        };

        let mut info = format!("mount_id: {}, type: {}", mount.mount_id(), prop_type_str);

        if let Some(group_id) = prop_info.shared_group_id {
            info.push_str(&format!(", shared_group: {}", group_id));
        }

        if prop_info.master.is_some() {
            info.push_str(", has_master: true");
        }

        if !prop_info.slaves.is_empty() {
            info.push_str(&format!(", slaves: {}", prop_info.slaves.len()));
        }

        info
    }

    /// 验证namespace中的传播一致性（用于调试）
    pub fn validate_propagation_consistency(&self) -> Result<(), SystemError> {
        log::info!("MountNamespace: validating propagation consistency");

        let inner = self.inner.lock();

        // 验证共享组的一致性
        for (group_id, group) in &inner.shared_groups {
            for member_weak in &group.members {
                if let Some(member) = member_weak.upgrade() {
                    let prop_info = member.get_propagation_info();
                    if prop_info.prop_type != PropagationType::Shared {
                        log::error!("MountNamespace: inconsistency found - mount {} in shared group {} but not shared type", 
                                   member.mount_id(), group_id);
                        return Err(SystemError::EINVAL);
                    }
                    if prop_info.shared_group_id != Some(*group_id) {
                        log::error!(
                            "MountNamespace: inconsistency found - mount {} group_id mismatch",
                            member.mount_id()
                        );
                        return Err(SystemError::EINVAL);
                    }

                    // 清理过期的slave引用
                    let _ = member.cleanup_stale_slaves();
                }
            }
        }

        // 使用传播引擎清理过期引用
        let engine = crate::filesystem::vfs::propagation::get_propagation_engine();
        engine.cleanup_stale_references();

        log::info!("MountNamespace: propagation consistency validation passed");
        Ok(())
    }

    /// 清理namespace中的过期引用
    pub fn cleanup_stale_references(&self) {
        log::debug!("MountNamespace: cleaning up stale references");

        let mut inner = self.inner.lock();

        // 检查namespace是否已标记为dead
        if inner.dead {
            log::warn!("MountNamespace: attempting cleanup on dead namespace");
            return;
        }

        // 清理所有共享组中的过期成员
        for group in inner.shared_groups.values_mut() {
            group.cleanup_stale_members();
        }

        // 清理所有挂载的slave引用
        let _mount_list = inner.mount_list.clone();
        drop(inner); // 释放锁

        // 遍历所有挂载点进行清理
        // 这里简化实现，在真实场景中需要遍历mount_list

        // 使传播引擎缓存失效
        let engine = crate::filesystem::vfs::propagation::get_propagation_engine();
        engine.invalidate_cache();

        log::debug!("MountNamespace: stale reference cleanup completed");
    }

    /// 标记namespace为dead状态
    pub fn mark_dead(&self) {
        let mut inner = self.inner.lock();
        inner.dead = true;
        log::info!("MountNamespace: marked as dead");
    }

    /// 检查namespace是否为dead状态
    pub fn is_dead(&self) -> bool {
        self.inner.lock().dead
    }

    /// 获取父namespace
    pub fn parent_namespace(&self) -> Option<Arc<MountNamespace>> {
        self.parent.as_ref()?.upgrade()
    }

    /// 获取用户namespace
    pub fn user_namespace(&self) -> Arc<crate::process::namespace::user_namespace::UserNamespace> {
        self.user_ns.clone()
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
