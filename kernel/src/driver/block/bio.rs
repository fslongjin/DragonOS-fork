use alloc::{sync::Arc, vec::Vec};
use core::ptr::NonNull;

use system_error::SystemError;

use crate::{
    arch::MMArch,
    driver::base::block::block_device::{BlockId, LBA_SIZE},
    libs::mutex::Mutex,
    mm::page::Page,
    mm::MemoryManagementArch,
    sched::completion::Completion,
};

/// Block IO 操作类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BioOp {
    Read,
    Write,
    Flush,
}

/// DMA 友好的 I/O 向量：指向一个物理页上的连续片段。
///
/// 约束：
/// - `offset + len <= PAGE_SIZE`
/// - 多个 `BioVec` 组合后的总长度应等于 `count * LBA_SIZE`
#[derive(Debug, Clone)]
pub struct BioVec {
    pub page: Arc<Page>,
    pub offset: usize,
    pub len: usize,
}

/// Bio 的数据载体。
///
/// - Borrowed*：仅用于“提交后立刻 wait”的同步封装路径（缓冲区生命周期由调用者保证）。
/// - Owned：用于真正的异步提交（缓冲区由 Bio 持有直到完成）。
#[derive(Debug)]
pub enum BioBuf {
    /// 只读借用（通常用于 Write）
    Borrowed {
        ptr: NonNull<u8>,
        len: usize,
    },
    /// 可写借用（通常用于 Read）
    BorrowedMut {
        ptr: NonNull<u8>,
        len: usize,
    },
    /// 持有型缓冲区
    Owned(Vec<u8>),
}

#[derive(Debug)]
pub enum BioData {
    Buf(BioBuf),
    Vec(Vec<BioVec>),
}

#[derive(Debug)]
pub struct Bio {
    pub op: BioOp,
    pub lba_id_start: BlockId,
    pub count: usize,
    data: Mutex<BioData>,
    completion: Completion,
    result: Mutex<Option<Result<usize, SystemError>>>,
}

impl Bio {
    #[inline]
    pub fn bytes_len(&self) -> usize {
        self.count.saturating_mul(LBA_SIZE)
    }

    pub fn new_read_owned(lba_id_start: BlockId, count: usize) -> Arc<Self> {
        Arc::new(Self {
            op: BioOp::Read,
            lba_id_start,
            count,
            data: Mutex::new(BioData::Buf(BioBuf::Owned(vec![0u8; count * LBA_SIZE]))),
            completion: Completion::new(),
            result: Mutex::new(None),
        })
    }

    pub fn new_write_owned(lba_id_start: BlockId, count: usize, data: &[u8]) -> Result<Arc<Self>, SystemError> {
        let bytes = count.checked_mul(LBA_SIZE).ok_or(SystemError::EOVERFLOW)?;
        if bytes > data.len() {
            return Err(SystemError::EINVAL);
        }
        Ok(Arc::new(Self {
            op: BioOp::Write,
            lba_id_start,
            count,
            data: Mutex::new(BioData::Buf(BioBuf::Owned(data[..bytes].to_vec()))),
            completion: Completion::new(),
            result: Mutex::new(None),
        }))
    }

    pub fn new_vec(op: BioOp, lba_id_start: BlockId, count: usize, vecs: Vec<BioVec>) -> Result<Arc<Self>, SystemError> {
        let bytes = count.checked_mul(LBA_SIZE).ok_or(SystemError::EOVERFLOW)?;
        let sum: usize = vecs.iter().map(|v| v.len).sum();
        if sum != bytes {
            return Err(SystemError::EINVAL);
        }
        Ok(Arc::new(Self {
            op,
            lba_id_start,
            count,
            data: Mutex::new(BioData::Vec(vecs)),
            completion: Completion::new(),
            result: Mutex::new(None),
        }))
    }

    /// 创建一个 Read Bio，直接写入调用者提供的缓冲区。
    ///
    /// # Safety
    /// 调用者必须保证 `buf` 在 Bio 完成前始终有效，且不会被并发可变访问。
    pub unsafe fn new_read_borrowed(lba_id_start: BlockId, count: usize, buf: &mut [u8]) -> Result<Arc<Self>, SystemError> {
        let bytes = count.checked_mul(LBA_SIZE).ok_or(SystemError::EOVERFLOW)?;
        if bytes > buf.len() {
            return Err(SystemError::EINVAL);
        }
        let ptr = NonNull::new(buf.as_mut_ptr()).ok_or(SystemError::EFAULT)?;
        Ok(Arc::new(Self {
            op: BioOp::Read,
            lba_id_start,
            count,
            data: Mutex::new(BioData::Buf(BioBuf::BorrowedMut { ptr, len: bytes })),
            completion: Completion::new(),
            result: Mutex::new(None),
        }))
    }

    /// 创建一个 Write Bio，直接读取调用者提供的缓冲区。
    ///
    /// # Safety
    /// 调用者必须保证 `buf` 在 Bio 完成前始终有效，且不会被并发修改。
    pub unsafe fn new_write_borrowed(lba_id_start: BlockId, count: usize, buf: &[u8]) -> Result<Arc<Self>, SystemError> {
        let bytes = count.checked_mul(LBA_SIZE).ok_or(SystemError::EOVERFLOW)?;
        if bytes > buf.len() {
            return Err(SystemError::EINVAL);
        }
        let ptr = NonNull::new(buf.as_ptr() as *mut u8).ok_or(SystemError::EFAULT)?;
        Ok(Arc::new(Self {
            op: BioOp::Write,
            lba_id_start,
            count,
            data: Mutex::new(BioData::Buf(BioBuf::Borrowed { ptr, len: bytes })),
            completion: Completion::new(),
            result: Mutex::new(None),
        }))
    }

    pub fn complete(&self, res: Result<usize, SystemError>) {
        *self.result.lock() = Some(res);
        self.completion.complete();
    }

    pub fn wait(&self) -> Result<usize, SystemError> {
        let _ = self.completion.wait_for_completion()?;
        self.result
            .lock()
            .take()
            .unwrap_or(Err(SystemError::EIO))
    }

    pub fn with_buf_readonly<R>(&self, f: impl FnOnce(&[u8]) -> R) -> Result<R, SystemError> {
        let guard = self.data.lock();
        match &*guard {
            BioData::Buf(b) => Ok(match b {
                BioBuf::Borrowed { ptr, len } => unsafe { f(core::slice::from_raw_parts(ptr.as_ptr(), *len)) },
                BioBuf::BorrowedMut { ptr, len } => unsafe { f(core::slice::from_raw_parts(ptr.as_ptr(), *len)) },
                BioBuf::Owned(v) => f(v.as_slice()),
            }),
            BioData::Vec(_) => Err(SystemError::EINVAL),
        }
    }

    pub fn with_buf_mut<R>(&self, f: impl FnOnce(&mut [u8]) -> R) -> Result<R, SystemError> {
        let mut guard = self.data.lock();
        match &mut *guard {
            BioData::Buf(b) => Ok(match b {
                BioBuf::Borrowed { ptr, len } => unsafe { f(core::slice::from_raw_parts_mut(ptr.as_ptr(), *len)) },
                BioBuf::BorrowedMut { ptr, len } => unsafe { f(core::slice::from_raw_parts_mut(ptr.as_ptr(), *len)) },
                BioBuf::Owned(v) => f(v.as_mut_slice()),
            }),
            BioData::Vec(_) => Err(SystemError::EINVAL),
        }
    }

    pub fn gather_to(&self, dst: &mut [u8]) -> Result<(), SystemError> {
        let bytes = self.bytes_len();
        if dst.len() < bytes {
            return Err(SystemError::EINVAL);
        }
        let guard = self.data.lock();
        match &*guard {
            BioData::Buf(b) => {
                let src = match b {
                    BioBuf::Borrowed { ptr, len } => unsafe { core::slice::from_raw_parts(ptr.as_ptr(), *len) },
                    BioBuf::BorrowedMut { ptr, len } => unsafe { core::slice::from_raw_parts(ptr.as_ptr(), *len) },
                    BioBuf::Owned(v) => v.as_slice(),
                };
                dst[..bytes].copy_from_slice(&src[..bytes]);
                Ok(())
            }
            BioData::Vec(vecs) => {
                let mut off = 0usize;
                for v in vecs {
                    let paddr = v.page.phys_address();
                    let src = unsafe {
                        core::slice::from_raw_parts(
                            MMArch::phys_2_virt(paddr).unwrap().data() as *const u8,
                            MMArch::PAGE_SIZE,
                        )
                    };
                    dst[off..off + v.len].copy_from_slice(&src[v.offset..v.offset + v.len]);
                    off += v.len;
                }
                Ok(())
            }
        }
    }

    pub fn scatter_from(&self, src: &[u8]) -> Result<(), SystemError> {
        let bytes = self.bytes_len();
        if src.len() < bytes {
            return Err(SystemError::EINVAL);
        }
        let mut guard = self.data.lock();
        match &mut *guard {
            BioData::Buf(b) => {
                match b {
                    BioBuf::Borrowed { ptr, len } => unsafe {
                        core::slice::from_raw_parts_mut(ptr.as_ptr(), *len)[..bytes]
                            .copy_from_slice(&src[..bytes]);
                    },
                    BioBuf::BorrowedMut { ptr, len } => unsafe {
                        core::slice::from_raw_parts_mut(ptr.as_ptr(), *len)[..bytes]
                            .copy_from_slice(&src[..bytes]);
                    },
                    BioBuf::Owned(v) => {
                        v[..bytes].copy_from_slice(&src[..bytes]);
                    }
                }
                Ok(())
            }
            BioData::Vec(vecs) => {
                let mut off = 0usize;
                for v in vecs {
                    let paddr = v.page.phys_address();
                    let dst = unsafe {
                        core::slice::from_raw_parts_mut(
                            MMArch::phys_2_virt(paddr).unwrap().data() as *mut u8,
                            MMArch::PAGE_SIZE,
                        )
                    };
                    dst[v.offset..v.offset + v.len].copy_from_slice(&src[off..off + v.len]);
                    off += v.len;
                }
                Ok(())
            }
        }
    }
}


