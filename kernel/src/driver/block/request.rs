use alloc::{sync::Arc, vec::Vec};

use system_error::SystemError;

use crate::driver::base::block::block_device::{BlockId, LBA_SIZE};

use super::bio::{Bio, BioOp};

/// 一次可调度的请求，可能由多个顺序 Bio 合并而成。
#[derive(Debug)]
pub struct Request {
    pub op: BioOp,
    pub lba_id_start: BlockId,
    pub count: usize,
    pub bios: Vec<Arc<Bio>>,
}

impl Request {
    pub fn new(first: Arc<Bio>) -> Self {
        Self {
            op: first.op,
            lba_id_start: first.lba_id_start,
            count: first.count,
            bios: vec![first],
        }
    }

    #[inline]
    pub fn lba_end(&self) -> BlockId {
        self.lba_id_start.saturating_add(self.count)
    }

    /// 是否可以把 `bio` 合并到当前 request 的尾部（顺序合并）。
    pub fn can_merge_tail(&self, bio: &Bio, max_merge_blocks: usize) -> bool {
        if self.op != bio.op {
            return false;
        }
        if self.lba_end() != bio.lba_id_start {
            return false;
        }
        self.count.saturating_add(bio.count) <= max_merge_blocks
    }

    pub fn merge_tail(&mut self, bio: Arc<Bio>) {
        self.count = self.count.saturating_add(bio.count);
        self.bios.push(bio);
    }

    #[inline]
    pub fn bytes_len(&self) -> usize {
        self.count.saturating_mul(LBA_SIZE)
    }

    /// 将 request 的写数据聚合成一段连续缓冲区（用于一次性下发）。
    pub fn gather_write_buf(&self) -> Result<Vec<u8>, SystemError> {
        if self.op != BioOp::Write {
            return Err(SystemError::EINVAL);
        }
        let mut out = vec![0u8; self.bytes_len()];
        let mut off = 0usize;
        for bio in &self.bios {
            let len = bio.bytes_len();
            bio.gather_to(&mut out[off..off + len])?;
            off += len;
        }
        Ok(out)
    }

    /// 将一次性读到的连续缓冲区分发到各个 Bio。
    pub fn scatter_read_buf(&self, data: &[u8]) -> Result<(), SystemError> {
        if self.op != BioOp::Read {
            return Err(SystemError::EINVAL);
        }
        if data.len() < self.bytes_len() {
            return Err(SystemError::EINVAL);
        }
        let mut off = 0usize;
        for bio in &self.bios {
            let len = bio.bytes_len();
            bio.scatter_from(&data[off..off + len])?;
            off += len;
        }
        Ok(())
    }
}


