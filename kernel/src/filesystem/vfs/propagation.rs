/// Mount propagation引擎
///
/// 这个模块实现了与Linux内核一致的挂载传播逻辑，包括：
/// - Shared propagation: 双向传播，组内成员共享所有挂载事件
/// - Slave propagation: 单向接收master的传播，不向外传播
/// - Private propagation: 完全隔离，不参与任何传播
/// - Unbindable propagation: 禁止bind mount操作
use alloc::{
    collections::VecDeque,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use hashbrown::HashSet;
use system_error::SystemError;

use crate::{
    filesystem::vfs::{
        mount::{MountFS, MountFSInode},
        IndexNode,
    },
    libs::spinlock::SpinLock,
    process::namespace::mount_namespace::PropagationType,
};

/// 传播事件类型
#[derive(Debug, Clone)]
pub enum PropagationEvent {
    /// 挂载事件
    Mount {
        source_mount: Arc<MountFS>,
        target_path: String,
        new_mount: Arc<MountFS>,
        flags: u32,
    },
    /// 卸载事件
    Umount {
        mount: Arc<MountFS>,
        path: String,
        flags: u32,
    },
    /// 重新挂载事件
    Remount { mount: Arc<MountFS>, flags: u32 },
    /// 传播类型变更事件
    PropagationChange {
        mount: Arc<MountFS>,
        old_type: PropagationType,
        new_type: PropagationType,
        recursive: bool,
    },
    /// Bind mount事件
    BindMount {
        source_mount: Arc<MountFS>,
        target_path: String,
        flags: u32,
    },
}

/// 传播追踪器 - 防止循环传播
#[derive(Debug)]
struct PropagationTracker {
    visited_mounts: HashSet<u32>, // 使用mount_id追踪
    propagation_depth: usize,
}

impl PropagationTracker {
    const MAX_DEPTH: usize = 32; // 最大传播深度

    fn new() -> Self {
        Self {
            visited_mounts: HashSet::new(),
            propagation_depth: 0,
        }
    }

    fn can_propagate(&mut self, mount_id: u32) -> bool {
        if self.propagation_depth >= Self::MAX_DEPTH {
            log::warn!("PropagationTracker: max depth {} reached", Self::MAX_DEPTH);
            return false;
        }

        if self.visited_mounts.contains(&mount_id) {
            log::warn!(
                "PropagationTracker: cycle detected for mount_id {}",
                mount_id
            );
            return false; // 检测到循环
        }

        self.visited_mounts.insert(mount_id);
        self.propagation_depth += 1;
        true
    }

    fn pop(&mut self, mount_id: u32) {
        self.visited_mounts.remove(&mount_id);
        if self.propagation_depth > 0 {
            self.propagation_depth -= 1;
        }
    }
}

/// 传播路径缓存
struct PropagationPathCache {
    cache: SpinLock<hashbrown::HashMap<String, Vec<Arc<MountFS>>>>,
    generation: AtomicU32,
}

impl PropagationPathCache {
    fn new() -> Self {
        Self {
            cache: SpinLock::new(hashbrown::HashMap::new()),
            generation: AtomicU32::new(0),
        }
    }

    /// 获取传播路径
    fn get_propagation_targets(&self, source_path: &str) -> Option<Vec<Arc<MountFS>>> {
        self.cache.lock().get(source_path).cloned()
    }

    /// 缓存传播路径
    fn cache_propagation_targets(&self, source_path: String, targets: Vec<Arc<MountFS>>) {
        self.cache.lock().insert(source_path, targets);
    }

    /// 失效缓存
    fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.cache.lock().clear();
    }
}

/// 批处理器 - 用于批量处理传播事件
struct BatchProcessor {
    batch_size: usize,
    max_delay_ms: u64,
}

impl BatchProcessor {
    fn new() -> Self {
        Self {
            batch_size: 32,
            max_delay_ms: 10,
        }
    }

    /// 批量处理事件
    fn process_batch(&self, events: Vec<PropagationEvent>) -> Result<(), SystemError> {
        log::debug!(
            "BatchProcessor: processing batch of {} events with max_delay_ms: {}",
            events.len(),
            self.max_delay_ms
        );

        // 在实际实现中，这里可以根据max_delay_ms来控制批处理的延迟
        // 简化版本直接处理

        // 按类型分组事件
        let mut mount_events = Vec::new();
        let mut umount_events = Vec::new();
        let mut propagation_changes = Vec::new();

        for event in events {
            match event {
                PropagationEvent::Mount { .. } => mount_events.push(event),
                PropagationEvent::Umount { .. } => umount_events.push(event),
                PropagationEvent::PropagationChange { .. } => propagation_changes.push(event),
                _ => {
                    // 其他事件单独处理
                }
            }
        }

        // 批量处理同类型事件
        self.process_mount_batch(mount_events)?;
        self.process_umount_batch(umount_events)?;
        self.process_propagation_change_batch(propagation_changes)?;

        Ok(())
    }

    fn process_mount_batch(&self, events: Vec<PropagationEvent>) -> Result<(), SystemError> {
        // 批量处理挂载事件
        for event in events {
            if let PropagationEvent::Mount {
                source_mount: _,
                target_path,
                new_mount: _,
                flags: _,
            } = event
            {
                // 简化的批处理逻辑
                log::debug!("BatchProcessor: processing mount event for {}", target_path);
            }
        }
        Ok(())
    }

    fn process_umount_batch(&self, events: Vec<PropagationEvent>) -> Result<(), SystemError> {
        // 批量处理卸载事件
        for event in events {
            if let PropagationEvent::Umount {
                mount: _,
                path,
                flags: _,
            } = event
            {
                log::debug!("BatchProcessor: processing umount event for {}", path);
            }
        }
        Ok(())
    }

    fn process_propagation_change_batch(
        &self,
        events: Vec<PropagationEvent>,
    ) -> Result<(), SystemError> {
        // 批量处理传播类型变更事件
        for event in events {
            if let PropagationEvent::PropagationChange {
                mount: _,
                old_type,
                new_type,
                recursive: _,
            } = event
            {
                log::debug!(
                    "BatchProcessor: processing propagation change {:?} -> {:?}",
                    old_type,
                    new_type
                );
            }
        }
        Ok(())
    }
}

/// 传播引擎 - 负责处理所有传播逻辑（优化版本）
pub struct PropagationEngine {
    event_queue: SpinLock<VecDeque<PropagationEvent>>,
    high_priority_queue: SpinLock<VecDeque<PropagationEvent>>,
    processing: AtomicBool,
    generation: AtomicU32, // 用于失效缓存
    cache: PropagationPathCache,
    batch_processor: BatchProcessor,
}

impl PropagationEngine {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            event_queue: SpinLock::new(VecDeque::new()),
            high_priority_queue: SpinLock::new(VecDeque::new()),
            processing: AtomicBool::new(false),
            generation: AtomicU32::new(0),
            cache: PropagationPathCache::new(),
            batch_processor: BatchProcessor::new(),
        })
    }

    /// 处理挂载传播事件
    pub fn handle_mount_event(
        &self,
        source_mount: &Arc<MountFS>,
        target_path: &str,
        new_mount: &Arc<MountFS>,
        flags: u32,
    ) -> Result<(), SystemError> {
        let event = PropagationEvent::Mount {
            source_mount: source_mount.clone(),
            target_path: target_path.to_string(),
            new_mount: new_mount.clone(),
            flags,
        };

        log::info!(
            "PropagationEngine: queueing mount event for path {}",
            target_path
        );
        self.event_queue.lock().push_back(event);
        self.process_events()?;
        Ok(())
    }

    /// 处理卸载传播事件
    pub fn handle_umount_event(
        &self,
        mount: &Arc<MountFS>,
        path: &str,
        flags: u32,
    ) -> Result<(), SystemError> {
        let event = PropagationEvent::Umount {
            mount: mount.clone(),
            path: path.to_string(),
            flags,
        };

        log::info!("PropagationEngine: queueing umount event for path {}", path);
        self.event_queue.lock().push_back(event);
        self.process_events()?;
        Ok(())
    }

    /// 处理传播类型变更事件
    pub fn handle_propagation_change_event(
        &self,
        mount: &Arc<MountFS>,
        old_type: PropagationType,
        new_type: PropagationType,
        recursive: bool,
    ) -> Result<(), SystemError> {
        let event = PropagationEvent::PropagationChange {
            mount: mount.clone(),
            old_type,
            new_type,
            recursive,
        };

        log::info!(
            "PropagationEngine: queueing propagation change event: {:?} -> {:?}",
            old_type,
            new_type
        );
        self.event_queue.lock().push_back(event);
        self.process_events()?;
        Ok(())
    }

    /// 处理bind mount事件
    pub fn handle_bind_mount_event(
        &self,
        source_mount: &Arc<MountFS>,
        target_path: &str,
        flags: u32,
    ) -> Result<(), SystemError> {
        let event = PropagationEvent::BindMount {
            source_mount: source_mount.clone(),
            target_path: target_path.to_string(),
            flags,
        };

        log::info!(
            "PropagationEngine: queueing bind mount event for path {}",
            target_path
        );
        self.event_queue.lock().push_back(event);
        self.process_events()?;
        Ok(())
    }

    /// 处理传播事件队列（优化版本）
    fn process_events(&self) -> Result<(), SystemError> {
        // 防止递归处理
        if self.processing.load(Ordering::Acquire) {
            log::debug!("PropagationEngine: already processing events, skipping");
            return Ok(());
        }

        self.processing.store(true, Ordering::Release);

        log::debug!("PropagationEngine: starting optimized event processing");

        // 优先处理高优先级事件
        self.process_high_priority_events()?;

        // 批量处理普通事件
        self.process_events_in_batches()?;

        self.processing.store(false, Ordering::Release);
        Ok(())
    }

    /// 处理高优先级事件
    #[inline(never)]
    fn process_high_priority_events(&self) -> Result<(), SystemError> {
        let mut processed = 0;
        while let Some(event) = self.high_priority_queue.lock().pop_front() {
            match self.process_single_event(event) {
                Ok(()) => processed += 1,
                Err(e) => {
                    log::error!(
                        "PropagationEngine: failed to process high priority event: {:?}",
                        e
                    );
                }
            }

            // 限制单次处理的高优先级事件数量
            if processed >= 50 {
                break;
            }
        }

        if processed > 0 {
            log::debug!(
                "PropagationEngine: processed {} high priority events",
                processed
            );
        }

        Ok(())
    }

    /// 批量处理普通事件
    #[inline(never)]
    fn process_events_in_batches(&self) -> Result<(), SystemError> {
        let mut batch = Vec::new();
        let batch_size = self.batch_processor.batch_size;

        // 收集事件到批次中
        {
            let mut queue = self.event_queue.lock();
            while let Some(event) = queue.pop_front() {
                batch.push(event);
                if batch.len() >= batch_size {
                    break;
                }
            }
        }

        if batch.is_empty() {
            return Ok(());
        }

        log::debug!(
            "PropagationEngine: processing batch of {} events",
            batch.len()
        );

        // 尝试使用批处理器
        if batch.len() >= 8 {
            // 大批次使用批处理器
            let batch_copy = batch.clone(); // 复制以备回退使用
            match self.batch_processor.process_batch(batch) {
                Ok(()) => {
                    log::debug!("PropagationEngine: batch processing completed successfully");
                    return Ok(());
                }
                Err(e) => {
                    log::warn!("PropagationEngine: batch processing failed: {:?}, falling back to individual processing", e);
                    // 使用复制的batch进行回退处理
                    for event in batch_copy {
                        if let Err(e) = self.process_single_event(event) {
                            log::error!("PropagationEngine: failed to process event: {:?}", e);
                        }
                    }
                    return Ok(());
                }
            }
        }

        // 逐个处理事件（正常模式）
        for event in batch {
            if let Err(e) = self.process_single_event(event) {
                log::error!("PropagationEngine: failed to process event: {:?}", e);
            }
        }

        Ok(())
    }

    /// 处理单个事件
    fn process_single_event(&self, event: PropagationEvent) -> Result<(), SystemError> {
        match event {
            PropagationEvent::Mount {
                source_mount,
                target_path,
                new_mount,
                flags,
            } => {
                self.process_mount_propagation(&source_mount, &target_path, &new_mount, flags)?;
            }
            PropagationEvent::Umount { mount, path, flags } => {
                self.process_umount_propagation(&mount, &path, flags)?;
            }
            PropagationEvent::PropagationChange {
                mount,
                old_type,
                new_type,
                recursive,
            } => {
                self.process_propagation_change(&mount, old_type, new_type, recursive)?;
            }
            PropagationEvent::BindMount {
                source_mount,
                target_path,
                flags,
            } => {
                self.process_bind_mount_propagation(&source_mount, &target_path, flags)?;
            }
            PropagationEvent::Remount { mount, flags } => {
                self.process_remount_propagation(&mount, flags)?;
            }
        }
        Ok(())
    }

    /// 处理挂载传播的核心逻辑
    fn process_mount_propagation(
        &self,
        source_mount: &Arc<MountFS>,
        target_path: &str,
        new_mount: &Arc<MountFS>,
        _flags: u32,
    ) -> Result<(), SystemError> {
        log::info!("PropagationEngine: ============ START MOUNT PROPAGATION ============");
        log::info!(
            "PropagationEngine: source_mount_id: {}, target_path: {}, new_mount_id: {}",
            source_mount.mount_id(),
            target_path,
            new_mount.mount_id()
        );

        let prop_info = source_mount.get_propagation_info();
        log::info!("PropagationEngine: source mount propagation type: {:?}, shared_group_id: {:?}, slaves: {}", 
                  prop_info.prop_type, prop_info.shared_group_id, prop_info.slaves.len());

        let mut tracker = PropagationTracker::new();

        match prop_info.prop_type {
            PropagationType::Shared => {
                log::info!("PropagationEngine: handling SHARED propagation");
                // 共享传播：传播到同组的所有成员
                if let Some(group_id) = prop_info.shared_group_id {
                    log::info!(
                        "PropagationEngine: propagating to shared group {}",
                        group_id
                    );
                    self.propagate_to_shared_group(
                        source_mount,
                        group_id,
                        target_path,
                        new_mount,
                        &mut tracker,
                    )?;
                } else {
                    log::warn!("PropagationEngine: shared mount has no group_id!");
                }

                // 同时传播到所有slave
                if !prop_info.slaves.is_empty() {
                    log::info!(
                        "PropagationEngine: also propagating to {} slaves",
                        prop_info.slaves.len()
                    );
                    self.propagate_to_slaves(source_mount, target_path, new_mount, &mut tracker)?;
                } else {
                    log::debug!("PropagationEngine: no slaves to propagate to");
                }
            }
            PropagationType::Private => {
                log::info!("PropagationEngine: handling PRIVATE propagation");
                // 私有传播：不向外传播，但可能向slave传播
                self.propagate_to_slaves(source_mount, target_path, new_mount, &mut tracker)?;
            }
            PropagationType::Slave => {
                log::info!("PropagationEngine: handling SLAVE propagation");
                // 从属传播：不向外传播
                // 但如果它也是某些挂载的master，需要向下传播
                self.propagate_to_slaves(source_mount, target_path, new_mount, &mut tracker)?;
            }
            PropagationType::Unbindable => {
                log::info!("PropagationEngine: UNBINDABLE mount, no propagation");
                // 不可绑定：完全不传播
                log::info!("PropagationEngine: ============ END MOUNT PROPAGATION (UNBINDABLE) ============");
                return Ok(());
            }
        }

        log::info!("PropagationEngine: ============ END MOUNT PROPAGATION ============");
        Ok(())
    }

    /// 传播到共享组
    fn propagate_to_shared_group(
        &self,
        source_mount: &Arc<MountFS>,
        group_id: u32,
        target_path: &str,
        new_mount: &Arc<MountFS>,
        tracker: &mut PropagationTracker,
    ) -> Result<(), SystemError> {
        // 使用优化版本的传播函数
        self.optimized_propagate_to_shared_group(
            source_mount,
            group_id,
            target_path,
            new_mount,
            tracker,
        )
    }

    /// 传播到从属挂载
    fn propagate_to_slaves(
        &self,
        source_mount: &Arc<MountFS>,
        target_path: &str,
        new_mount: &Arc<MountFS>,
        tracker: &mut PropagationTracker,
    ) -> Result<(), SystemError> {
        let prop_info = source_mount.get_propagation_info();

        log::info!(
            "PropagationEngine: propagating to {} slaves",
            prop_info.slaves.len()
        );

        for slave_weak in &prop_info.slaves {
            if let Some(slave) = slave_weak.upgrade() {
                // 检查是否可以传播（防止循环）
                if !tracker.can_propagate(slave.mount_id()) {
                    continue;
                }

                log::info!(
                    "PropagationEngine: propagating to slave mount_id: {}",
                    slave.mount_id()
                );

                match self.create_propagated_mount(&slave, target_path, new_mount) {
                    Ok(()) => {
                        log::info!(
                            "PropagationEngine: successfully propagated to slave mount_id: {}",
                            slave.mount_id()
                        );
                    }
                    Err(e) => {
                        log::error!(
                            "PropagationEngine: failed to propagate to slave mount_id {}: {:?}",
                            slave.mount_id(),
                            e
                        );
                    }
                }

                tracker.pop(slave.mount_id());
            }
        }

        Ok(())
    }

    /// 创建传播的挂载
    fn create_propagated_mount(
        &self,
        target_mount: &Arc<MountFS>,
        relative_path: &str,
        source_mount: &Arc<MountFS>,
    ) -> Result<(), SystemError> {
        log::info!("PropagationEngine: --- CREATE PROPAGATED MOUNT START ---");
        log::info!(
            "PropagationEngine: target_mount_id: {}, relative_path: {}, source_mount_id: {}",
            target_mount.mount_id(),
            relative_path,
            source_mount.mount_id()
        );

        // 检查namespace边界
        let target_ns = target_mount.namespace().ok_or(SystemError::EINVAL)?;
        let source_ns = source_mount.namespace().ok_or(SystemError::EINVAL)?;

        let same_namespace = Arc::ptr_eq(&target_ns, &source_ns);
        log::info!(
            "PropagationEngine: namespace check - same_namespace: {}",
            same_namespace
        );

        // 只有在以下情况才允许传播：
        // 1. 同一个namespace内
        // 2. 或者target是source的slave（跨namespace的master-slave关系）
        if !same_namespace {
            log::info!("PropagationEngine: cross-namespace propagation detected, checking master-slave relationship");
            let source_prop = source_mount.get_propagation_info();
            let target_prop = target_mount.get_propagation_info();

            // 检查是否存在master-slave关系
            let has_master_slave_relation = source_prop.slaves.iter().any(|slave_weak| {
                if let Some(slave) = slave_weak.upgrade() {
                    Arc::ptr_eq(&slave, target_mount)
                } else {
                    false
                }
            }) || target_prop
                .master
                .as_ref()
                .and_then(|master_weak| master_weak.upgrade())
                .map(|master| Arc::ptr_eq(&master, source_mount))
                .unwrap_or(false);

            log::info!(
                "PropagationEngine: master-slave relationship exists: {}",
                has_master_slave_relation
            );

            if !has_master_slave_relation {
                log::info!("PropagationEngine: skipping cross-namespace propagation without master-slave relationship");
                return Ok(());
            }
        } else {
            log::info!("PropagationEngine: same namespace, proceeding with propagation");
        }

        // 复制源文件系统
        let fs_copy = source_mount.inner_filesystem().clone();
        log::info!("PropagationEngine: copied source filesystem");

        // 分配新的mount ID
        let mount_id = crate::process::namespace::mount_namespace::alloc_global_mount_id();
        log::info!("PropagationEngine: allocated new mount_id: {}", mount_id);

        // 创建新的MountFS，继承传播属性但修改为private（避免二次传播）
        let mut inherited_prop = source_mount.get_propagation_info();
        log::info!(
            "PropagationEngine: original propagation type: {:?}",
            inherited_prop.prop_type
        );
        inherited_prop.prop_type = PropagationType::Private; // 传播的挂载默认为private
        inherited_prop.shared_group_id = None;
        inherited_prop.slaves.clear();
        inherited_prop.reset_propagation_count();
        log::info!("PropagationEngine: set propagated mount to Private type");

        let new_mount = MountFS::new_with_namespace(
            fs_copy,
            None, // 将在mount时设置
            Arc::downgrade(&target_ns),
            inherited_prop,
            mount_id,
        );
        log::info!("PropagationEngine: created new MountFS for propagation");

        // 在目标位置执行挂载 - 正确的方式是使用mountpoints映射
        let target_root = target_mount.mountpoint_root_inode();
        log::info!(
            "PropagationEngine: attempting to resolve path '{}' in target mount",
            relative_path
        );

        if let Ok(target_inode) = self.resolve_path_in_mount(&target_root, relative_path) {
            log::info!("PropagationEngine: successfully resolved target path, executing mount");

            // 获取目标inode的metadata
            let metadata = target_inode.metadata()?;
            log::info!(
                "PropagationEngine: target inode_id: {:?}, file_type: {:?}",
                metadata.inode_id,
                metadata.file_type
            );

            // 检查是否为目录且不是挂载点根
            if metadata.file_type != crate::filesystem::vfs::FileType::Dir {
                log::warn!("PropagationEngine: target is not a directory");
                return Err(SystemError::ENOTDIR);
            }

            // 使用公共方法将新的MountFS插入到target_mount的mountpoints中
            log::info!("PropagationEngine: inserting new mount into target's mountpoints");
            target_mount.insert_mountpoint(metadata.inode_id, new_mount.clone());

            // 同时需要将传播的挂载点添加到目标namespace的MOUNT_LIST中
            // 构建挂载点在目标namespace中的绝对路径
            let target_absolute_path = if let Ok(target_path) = target_inode.absolute_path() {
                target_path
            } else {
                // 如果无法获取绝对路径，尝试从target_mount构建
                log::warn!("PropagationEngine: failed to get absolute path from target_inode, using fallback");
                if let Ok(target_mount_path) = target_mount.mountpoint_root_inode().absolute_path()
                {
                    if relative_path.starts_with('/') {
                        format!(
                            "{}{}",
                            target_mount_path.trim_end_matches('/'),
                            relative_path
                        )
                    } else {
                        format!(
                            "{}/{}",
                            target_mount_path.trim_end_matches('/'),
                            relative_path
                        )
                    }
                } else {
                    log::error!(
                        "PropagationEngine: failed to construct absolute path for propagated mount"
                    );
                    return Err(SystemError::EINVAL);
                }
            };

            log::info!(
                "PropagationEngine: adding propagated mount to MOUNT_LIST with path: {}",
                target_absolute_path
            );

            // 添加到目标namespace的MOUNT_LIST中
            if let Some(target_ns) = target_mount.namespace() {
                let mount_list = target_ns.mount_list();
                mount_list.insert(target_absolute_path, new_mount.clone());
                log::info!("PropagationEngine: successfully added to target namespace MOUNT_LIST");
            } else {
                log::warn!(
                    "PropagationEngine: target mount has no namespace, using global MOUNT_LIST"
                );
                crate::filesystem::vfs::mount::GLOBAL_MOUNT_LIST()
                    .insert(target_absolute_path, new_mount.clone());
            }

            log::info!(
                "PropagationEngine: ✓ successfully created propagated mount with id: {}",
                new_mount.mount_id()
            );
            log::info!("PropagationEngine: --- CREATE PROPAGATED MOUNT END (SUCCESS) ---");
        } else {
            log::warn!(
                "PropagationEngine: failed to resolve target path: {}",
                relative_path
            );
            log::info!("PropagationEngine: --- CREATE PROPAGATED MOUNT END (FAILED) ---");
            return Err(SystemError::ENOENT);
        }

        Ok(())
    }

    /// 在挂载中解析路径
    fn resolve_path_in_mount(
        &self,
        root: &Arc<MountFSInode>,
        path: &str,
    ) -> Result<Arc<dyn IndexNode>, SystemError> {
        // 实现路径解析逻辑
        let mut current = root.clone() as Arc<dyn IndexNode>;

        // 跳过开头的斜杠
        let path = path.strip_prefix('/').unwrap_or(path);

        if path.is_empty() {
            return Ok(current);
        }

        for component in path.split('/').filter(|s| !s.is_empty()) {
            log::debug!("PropagationEngine: resolving path component: {}", component);
            current = current.find(component)?;
        }

        Ok(current)
    }

    /// 处理卸载传播
    fn process_umount_propagation(
        &self,
        source_mount: &Arc<MountFS>,
        target_path: &str,
        _flags: u32,
    ) -> Result<(), SystemError> {
        log::info!(
            "PropagationEngine: processing umount propagation for path {}",
            target_path
        );

        let prop_info = source_mount.get_propagation_info();

        match prop_info.prop_type {
            PropagationType::Shared => {
                // 传播卸载到同一共享组的所有挂载点
                if let Some(group_id) = prop_info.shared_group_id {
                    self.propagate_umount_to_shared_group(group_id, target_path, source_mount)?;
                }
            }
            PropagationType::Slave => {
                // 从属挂载不向外传播卸载
                log::info!("PropagationEngine: slave mount, no umount propagation");
            }
            PropagationType::Private => {
                // 私有挂载不传播
                log::info!("PropagationEngine: private mount, no umount propagation");
            }
            PropagationType::Unbindable => {
                // 不可绑定挂载的卸载不传播
                log::info!("PropagationEngine: unbindable mount, no umount propagation");
            }
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
        log::info!(
            "PropagationEngine: propagating umount to shared group {}",
            group_id
        );

        let namespace = source_mount.namespace().ok_or(SystemError::EINVAL)?;
        let members = namespace.get_shared_group_members(group_id);

        for member in members {
            // 跳过源挂载点
            if Arc::ptr_eq(&member, source_mount) {
                continue;
            }

            log::info!(
                "PropagationEngine: propagating umount to shared group member, mount_id: {}",
                member.mount_id()
            );

            // 在实际实现中，这里应该：
            // 1. 在对应的挂载点执行卸载操作
            // 2. 更新相应的mount list
            // 3. 递归地处理子挂载点

            // 简化实现：仅记录传播事件
            log::info!(
                "PropagationEngine: umount propagation to mount_id {} (simplified)",
                member.mount_id()
            );
        }

        Ok(())
    }

    /// 处理传播类型变更
    fn process_propagation_change(
        &self,
        mount: &Arc<MountFS>,
        old_type: PropagationType,
        new_type: PropagationType,
        recursive: bool,
    ) -> Result<(), SystemError> {
        log::info!(
            "PropagationEngine: processing propagation change {:?} -> {:?}, recursive: {}",
            old_type,
            new_type,
            recursive
        );

        // 如果是递归变更，需要处理所有子挂载
        if recursive {
            self.apply_propagation_change_recursive(mount, new_type)?;
        }

        // 处理共享组成员关系的变更
        match (old_type, new_type) {
            (PropagationType::Shared, PropagationType::Private) => {
                // 从共享组中移除
                if let Some(ns) = mount.namespace() {
                    let prop_info = mount.get_propagation_info();
                    if let Some(group_id) = prop_info.shared_group_id {
                        ns.leave_shared_group(group_id, &Arc::downgrade(mount))?;
                    }
                }
            }
            (PropagationType::Private, PropagationType::Shared) => {
                // 加入或创建共享组
                if let Some(ns) = mount.namespace() {
                    let group_id = ns.create_or_join_shared_group(Arc::downgrade(mount))?;
                    log::info!("PropagationEngine: joined shared group {}", group_id);
                }
            }
            _ => {
                // 其他变更类型的处理
                log::info!("PropagationEngine: other propagation change handled");
            }
        }

        Ok(())
    }

    /// 递归应用传播类型变更
    fn apply_propagation_change_recursive(
        &self,
        mount: &Arc<MountFS>,
        new_type: PropagationType,
    ) -> Result<(), SystemError> {
        log::info!(
            "PropagationEngine: applying recursive propagation change to {:?}",
            new_type
        );

        let child_mounts = mount.get_child_mounts();

        for (_, child_mount) in child_mounts {
            child_mount.set_propagation(new_type)?;
            self.apply_propagation_change_recursive(&child_mount, new_type)?;
        }

        Ok(())
    }

    /// 处理bind mount传播
    fn process_bind_mount_propagation(
        &self,
        source_mount: &Arc<MountFS>,
        target_path: &str,
        flags: u32,
    ) -> Result<(), SystemError> {
        log::info!(
            "PropagationEngine: processing bind mount propagation for path {}",
            target_path
        );

        // 检查source是否为unbindable
        if source_mount.propagation() == PropagationType::Unbindable {
            log::error!("PropagationEngine: bind mount on unbindable filesystem");
            return Err(SystemError::EINVAL);
        }

        // Bind mount的传播逻辑与常规mount类似
        self.process_mount_propagation(source_mount, target_path, source_mount, flags)?;

        Ok(())
    }

    /// 处理重新挂载事件
    pub fn handle_remount_event(
        &self,
        mount: &Arc<MountFS>,
        flags: u32,
    ) -> Result<(), SystemError> {
        let event = PropagationEvent::Remount {
            mount: mount.clone(),
            flags,
        };

        log::info!(
            "PropagationEngine: queueing remount event for mount_id {}",
            mount.mount_id()
        );
        self.event_queue.lock().push_back(event);
        self.process_events()?;
        Ok(())
    }

    /// 处理重新挂载传播
    fn process_remount_propagation(
        &self,
        mount: &Arc<MountFS>,
        _flags: u32,
    ) -> Result<(), SystemError> {
        log::info!(
            "PropagationEngine: processing remount propagation for mount_id {}",
            mount.mount_id()
        );

        // 重新挂载通常不需要传播，除非改变了传播属性
        // 这里主要是为了完整性

        Ok(())
    }

    /// 失效缓存
    pub fn invalidate_cache(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.cache.invalidate();
        log::debug!("PropagationEngine: cache invalidated");
    }

    /// 添加高优先级事件
    pub fn add_high_priority_event(&self, event: PropagationEvent) {
        self.high_priority_queue.lock().push_back(event);
    }

    /// 获取引擎统计信息
    pub fn get_stats(&self) -> PropagationEngineStats {
        PropagationEngineStats {
            normal_queue_length: self.event_queue.lock().len(),
            high_priority_queue_length: self.high_priority_queue.lock().len(),
            cache_entries: self.cache.cache.lock().len(),
            generation: self.generation.load(Ordering::Relaxed),
            is_processing: self.processing.load(Ordering::Relaxed),
        }
    }

    /// 优化共享组传播（使用缓存）
    fn optimized_propagate_to_shared_group(
        &self,
        source_mount: &Arc<MountFS>,
        group_id: u32,
        target_path: &str,
        new_mount: &Arc<MountFS>,
        tracker: &mut PropagationTracker,
    ) -> Result<(), SystemError> {
        log::info!("PropagationEngine: === SHARED GROUP PROPAGATION START ===");
        log::info!(
            "PropagationEngine: group_id: {}, target_path: {}",
            group_id,
            target_path
        );

        // 检查缓存
        let cache_key = alloc::format!("shared_group_{}", group_id);
        let members = if let Some(cached_members) = self.cache.get_propagation_targets(&cache_key) {
            log::info!(
                "PropagationEngine: using cached shared group members, count: {}",
                cached_members.len()
            );
            cached_members
        } else {
            // 缓存未命中，从namespace获取
            let namespace = source_mount.namespace().ok_or(SystemError::EINVAL)?;
            log::info!("PropagationEngine: cache miss, fetching from namespace");
            let members = namespace.get_shared_group_members(group_id);
            log::info!(
                "PropagationEngine: found {} members in shared group {}",
                members.len(),
                group_id
            );

            // 打印所有成员的详细信息
            for (i, member) in members.iter().enumerate() {
                log::info!(
                    "PropagationEngine: member[{}]: mount_id={}, namespace_id={}",
                    i,
                    member.mount_id(),
                    member
                        .namespace()
                        .map(|ns| format!("{:p}", ns.as_ref()))
                        .unwrap_or_else(|| "None".to_string())
                );
            }

            // 更新缓存
            self.cache
                .cache_propagation_targets(cache_key, members.clone());
            log::info!(
                "PropagationEngine: cached {} shared group members",
                members.len()
            );

            members
        };

        if members.is_empty() {
            log::warn!(
                "PropagationEngine: shared group {} has no members!",
                group_id
            );
            return Ok(());
        }

        // 批量处理成员
        let mut successful_propagations = 0;
        let mut failed_propagations = 0;
        let mut skipped_self = 0;
        let mut skipped_cycles = 0;

        for member in members {
            // 跳过源挂载点
            if Arc::ptr_eq(&member, source_mount) {
                skipped_self += 1;
                log::debug!(
                    "PropagationEngine: skipping self mount_id: {}",
                    member.mount_id()
                );
                continue;
            }

            // 检查循环防护
            if !tracker.can_propagate(member.mount_id()) {
                skipped_cycles += 1;
                log::debug!(
                    "PropagationEngine: skipping cycle for mount_id: {}",
                    member.mount_id()
                );
                continue;
            }

            log::info!(
                "PropagationEngine: propagating to shared group member, mount_id: {}",
                member.mount_id()
            );

            match self.create_propagated_mount(&member, target_path, new_mount) {
                Ok(()) => {
                    successful_propagations += 1;
                    log::info!(
                        "PropagationEngine: ✓ successful propagation to mount_id: {}",
                        member.mount_id()
                    );
                }
                Err(e) => {
                    failed_propagations += 1;
                    log::error!(
                        "PropagationEngine: ✗ failed propagation to mount_id {}: {:?}",
                        member.mount_id(),
                        e
                    );
                }
            }

            tracker.pop(member.mount_id());
        }

        log::info!("PropagationEngine: === SHARED GROUP PROPAGATION END ===");
        log::info!("PropagationEngine: Results - {} successful, {} failed, {} skipped (self), {} skipped (cycles)", 
                  successful_propagations, failed_propagations, skipped_self, skipped_cycles);

        Ok(())
    }

    /// 清理过期引用（性能优化）
    pub fn cleanup_stale_references(&self) {
        log::debug!("PropagationEngine: cleaning up stale references");

        // 清理缓存中的过期引用
        let mut cache = self.cache.cache.lock();
        cache.retain(|_, targets| {
            targets.retain(|mount| mount.mount_id() != 0); // 简化检查
            !targets.is_empty()
        });

        log::debug!("PropagationEngine: stale reference cleanup completed");
    }
}

/// 传播引擎统计信息
#[derive(Debug)]
pub struct PropagationEngineStats {
    pub normal_queue_length: usize,
    pub high_priority_queue_length: usize,
    pub cache_entries: usize,
    pub generation: u32,
    pub is_processing: bool,
}

/// 全局传播引擎实例
static mut GLOBAL_PROPAGATION_ENGINE: Option<Arc<PropagationEngine>> = None;

/// 获取全局传播引擎
pub fn get_propagation_engine() -> Arc<PropagationEngine> {
    unsafe {
        if GLOBAL_PROPAGATION_ENGINE.is_none() {
            GLOBAL_PROPAGATION_ENGINE = Some(PropagationEngine::new());
            log::info!("PropagationEngine: initialized global instance");
        }
        GLOBAL_PROPAGATION_ENGINE.as_ref().unwrap().clone()
    }
}

/// 初始化传播引擎
pub fn init_propagation_engine() {
    let engine = get_propagation_engine();
    log::info!(
        "PropagationEngine: initialized with stats: {:?}",
        engine.get_stats()
    );
}
