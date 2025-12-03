//! 时间片轮转调度类 (Round-Robin)
//!
//! MVP 核心实现，使用简单的 VecDeque 作为就绪队列。
//! O(1) 入队出队，公平地分配 CPU 时间。

use alloc::{collections::VecDeque, sync::Arc};
use core::time::Duration;

use super::{EnqueueFlags, SchedClassRq, SchedEntity, UpdateFlags};

/// 时间片轮转调度运行队列
#[derive(Debug)]
pub struct RoundRobinClassRq {
    /// 就绪队列（VecDeque 实现 O(1) 入队出队）
    queue: VecDeque<Arc<SchedEntity>>,
}

impl RoundRobinClassRq {
    /// 创建新的 RR 调度队列
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
        }
    }

    /// 检查实体是否在队列中
    pub fn contains(&self, entity: &Arc<SchedEntity>) -> bool {
        self.queue.iter().any(|e| Arc::ptr_eq(e, entity))
    }
}

impl Default for RoundRobinClassRq {
    fn default() -> Self {
        Self::new()
    }
}

impl SchedClassRq for RoundRobinClassRq {
    fn enqueue(&mut self, entity: Arc<SchedEntity>, _flags: EnqueueFlags) {
        // 避免重复入队
        if self.contains(&entity) {
            return;
        }

        // 重置时间片
        entity.reset_slice();

        // 加入队尾
        self.queue.push_back(entity);
    }

    fn dequeue(&mut self, entity: &Arc<SchedEntity>) {
        // 从队列中移除
        self.queue.retain(|e| !Arc::ptr_eq(e, entity));
    }

    fn pick_next(&mut self) -> Option<Arc<SchedEntity>> {
        // 从队首取出
        let entity = self.queue.pop_front()?;
        
        // 调试：检查是否选中了退出的任务
        if entity.state().is_exited() {
            let pid = entity.pcb().map(|p| p.raw_pid().data()).unwrap_or(9999);
            log::error!(
                "RR pick_next: selected EXITED task! pid={} queue_len={}",
                pid,
                self.queue.len()
            );
        }
        
        Some(entity)
    }

    fn put_prev(&mut self, entity: Arc<SchedEntity>) {
        // 如果时间片还没用完，放到队尾继续运行
        // 如果时间片用完了，也放到队尾（轮转）
        if !self.contains(&entity) && entity.state().is_runnable() {
            // 重置时间片
            entity.reset_slice();
            self.queue.push_back(entity);
        }
    }

    fn update_current(
        &mut self,
        entity: &Arc<SchedEntity>,
        delta: Duration,
        flags: UpdateFlags,
    ) -> bool {
        // 主动让出
        if flags.contains(UpdateFlags::YIELD) {
            return true;
        }

        // 进入等待或退出
        if flags.contains(UpdateFlags::WAIT) || flags.contains(UpdateFlags::EXIT) {
            return true;
        }

        // 时钟 tick：检查时间片是否用完
        if flags.contains(UpdateFlags::TICK) {
            let expired = entity.charge_runtime(delta.as_nanos() as u64);
            if expired {
                return true;
            }
        }

        false
    }

    fn has_runnable(&self) -> bool {
        !self.queue.is_empty()
    }

    fn len(&self) -> usize {
        self.queue.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Weak;

    fn create_test_entity() -> Arc<SchedEntity> {
        SchedEntity::new(Weak::new())
    }

    #[test]
    fn test_enqueue_dequeue() {
        let mut rq = RoundRobinClassRq::new();
        let entity = create_test_entity();

        assert!(rq.is_empty());
        rq.enqueue(entity.clone(), EnqueueFlags::SPAWN);
        assert_eq!(rq.len(), 1);
        assert!(rq.has_runnable());

        let picked = rq.pick_next();
        assert!(picked.is_some());
        assert!(Arc::ptr_eq(&picked.unwrap(), &entity));
        assert!(rq.is_empty());
    }

    #[test]
    fn test_no_duplicate_enqueue() {
        let mut rq = RoundRobinClassRq::new();
        let entity = create_test_entity();

        rq.enqueue(entity.clone(), EnqueueFlags::SPAWN);
        rq.enqueue(entity.clone(), EnqueueFlags::SPAWN);
        assert_eq!(rq.len(), 1);
    }

    #[test]
    fn test_fifo_order() {
        let mut rq = RoundRobinClassRq::new();
        let e1 = create_test_entity();
        let e2 = create_test_entity();
        let e3 = create_test_entity();

        rq.enqueue(e1.clone(), EnqueueFlags::SPAWN);
        rq.enqueue(e2.clone(), EnqueueFlags::SPAWN);
        rq.enqueue(e3.clone(), EnqueueFlags::SPAWN);

        assert!(Arc::ptr_eq(&rq.pick_next().unwrap(), &e1));
        assert!(Arc::ptr_eq(&rq.pick_next().unwrap(), &e2));
        assert!(Arc::ptr_eq(&rq.pick_next().unwrap(), &e3));
    }
}
