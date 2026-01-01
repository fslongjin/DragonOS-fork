use alloc::{boxed::Box, collections::VecDeque, string::String, sync::Arc};
use core::sync::atomic::{AtomicBool, Ordering};

use system_error::SystemError;

use crate::{
    driver::base::block::block_device::BlockId,
    libs::{spinlock::SpinLock, wait_queue::WaitQueue},
    process::kthread::{KernelThreadClosure, KernelThreadMechanism},
};

use super::super::{
    bio::{Bio, BioOp},
    request::Request,
};

/// 设备后端：负责把一个“连续的 LBA 区间 + 连续缓冲区”真正下发给硬件。
pub trait BlockIoBackend: Send + Sync + 'static {
    fn read(&self, lba_id_start: BlockId, count: usize, buf: &mut [u8]) -> Result<usize, SystemError>;
    fn write(&self, lba_id_start: BlockId, count: usize, buf: &[u8]) -> Result<usize, SystemError>;
    fn flush(&self) -> Result<(), SystemError>;
}

/// 最简单的调度器：FIFO + 顺序合并。
///
/// - 合并策略：同 op 且 LBA 连续，最多合并到 `max_merge_blocks`。
/// - 执行模型：一个 kthread 从队列取 request，聚合为一次设备 IO，完成后分发到各 Bio。
pub struct NoopRequestQueue {
    backend: Arc<dyn BlockIoBackend>,
    max_merge_blocks: usize,
    queue: SpinLock<VecDeque<Request>>,
    wq: WaitQueue,
    has_pending: AtomicBool,
}

impl NoopRequestQueue {
    pub fn new(backend: Arc<dyn BlockIoBackend>, max_merge_blocks: usize, thread_name: String) -> Arc<Self> {
        let q = Arc::new(Self {
            backend,
            max_merge_blocks,
            queue: SpinLock::new(VecDeque::new()),
            wq: WaitQueue::default(),
            has_pending: AtomicBool::new(false),
        });

        // 启动 worker kthread
        let arg = Arc::into_raw(q.clone()) as usize;
        let closure = KernelThreadClosure::UsizeClosure((
            Box::new(|arg| -> i32 {
                let q = unsafe { Arc::from_raw(arg as *const NoopRequestQueue) };
                // ! 会自动转换为 i32；该线程不应返回
                q.worker_loop()
            }),
            arg,
        ));
        KernelThreadMechanism::create_and_run(closure, thread_name);

        q
    }

    pub fn submit(&self, bio: Arc<Bio>) {
        {
            let mut guard = self.queue.lock();
            if let Some(tail) = guard.back_mut() {
                if tail.can_merge_tail(&bio, self.max_merge_blocks) {
                    tail.merge_tail(bio);
                } else {
                    guard.push_back(Request::new(bio));
                }
            } else {
                guard.push_back(Request::new(bio));
            }
        }
        self.has_pending.store(true, Ordering::Release);
        self.wq.wakeup(None);
    }

    fn pop_request(&self) -> Option<Request> {
        let mut guard = self.queue.lock();
        let r = guard.pop_front();
        if guard.is_empty() {
            self.has_pending.store(false, Ordering::Release);
        }
        r
    }

    fn worker_loop(&self) -> ! {
        loop {
            // 没活就睡，等 submit 唤醒
            if !self.has_pending.load(Ordering::Acquire) {
                // 直接使用 WaitQueue API，避免依赖宏的导入可见性问题
                let _ = self.wq.wait_event_interruptible_timeout(
                    || self.has_pending.load(Ordering::Acquire),
                    None,
                );
            }

            let Some(req) = self.pop_request() else {
                continue;
            };
            self.handle_request(req);
        }
    }

    fn handle_request(&self, req: Request) {
        match req.op {
            BioOp::Read => {
                let mut data = vec![0u8; req.bytes_len()];
                let io_res = self
                    .backend
                    .read(req.lba_id_start, req.count, &mut data[..])
                    .map(|_| req.bytes_len());

                match io_res {
                    Ok(_) => {
                        let scatter_res = req.scatter_read_buf(&data);
                        if scatter_res.is_err() {
                            for bio in req.bios {
                                bio.complete(Err(SystemError::EIO));
                            }
                            return;
                        }
                        for bio in req.bios {
                            bio.complete(Ok(bio.bytes_len()));
                        }
                    }
                    Err(e) => {
                        for bio in req.bios {
                            bio.complete(Err(e.clone()));
                        }
                    }
                }
            }
            BioOp::Write => {
                let gathered = req.gather_write_buf();
                let io_res = match gathered {
                    Ok(buf) => self.backend.write(req.lba_id_start, req.count, &buf).map(|_| req.bytes_len()),
                    Err(e) => Err(e),
                };

                match io_res {
                    Ok(_) => {
                        for bio in req.bios {
                            bio.complete(Ok(bio.bytes_len()));
                        }
                    }
                    Err(e) => {
                        for bio in req.bios {
                            bio.complete(Err(e.clone()));
                        }
                    }
                }
            }
            BioOp::Flush => {
                let r = self.backend.flush().map(|_| 0);
                for bio in req.bios {
                    bio.complete(r.clone());
                }
            }
        }
    }
}


