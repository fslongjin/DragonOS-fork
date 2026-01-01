use core::{
    cmp::min,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};
use hashbrown::HashMap;
use system_error::SystemError;

use super::vfs::IndexNode;
use crate::driver::block::bio::Bio;
use crate::libs::mutex::{Mutex, MutexGuard};
use crate::mm::page::FileMapInfo;
use crate::{arch::mm::LockedFrameAllocator, libs::lazy_init::Lazy};
use crate::{
    arch::MMArch,
    mm::{
        page::{page_manager_lock_irqsave, page_reclaimer_lock, Page, PageFlags},
        MemoryManagementArch,
    },
};
use crate::{libs::align::page_align_up, mm::page::PageType};

static PAGE_CACHE_ID: AtomicUsize = AtomicUsize::new(0);
/// 页面缓存
#[derive(Debug)]
pub struct PageCache {
    id: usize,
    inner: Mutex<InnerPageCache>,
    inode: Lazy<Weak<dyn IndexNode>>,
    unevictable: AtomicBool,
}

#[derive(Debug)]
pub struct InnerPageCache {
    #[allow(unused)]
    id: usize,
    pages: HashMap<usize, Arc<Page>>,
    page_cache_ref: Weak<PageCache>,
}

/// 描述一次从页缓存到目标缓冲区的拷贝
pub struct CopyItem {
    page: Arc<Page>,
    page_offset: usize,
    sub_len: usize,
}

impl InnerPageCache {
    pub fn new(page_cache_ref: Weak<PageCache>, id: usize) -> InnerPageCache {
        Self {
            id,
            pages: HashMap::new(),
            page_cache_ref,
        }
    }

    pub fn add_page(&mut self, offset: usize, page: &Arc<Page>) {
        self.pages.insert(offset, page.clone());
    }

    pub fn get_page(&self, offset: usize) -> Option<Arc<Page>> {
        self.pages.get(&offset).cloned()
    }

    pub fn remove_page(&mut self, offset: usize) -> Option<Arc<Page>> {
        self.pages.remove(&offset)
    }

    pub fn create_pages(&mut self, start_page_index: usize, buf: &[u8]) -> Result<(), SystemError> {
        if buf.is_empty() {
            return Ok(());
        }

        let page_num = ((buf.len() - 1) >> MMArch::PAGE_SHIFT) + 1;

        let mut page_manager_guard = page_manager_lock_irqsave();

        for i in 0..page_num {
            let buf_offset = i * MMArch::PAGE_SIZE;
            let page_index = start_page_index + i;

            let page_flags = {
                let cache = self
                    .page_cache_ref
                    .upgrade()
                    .expect("failed to get self_arc of pagecache");
                if cache.unevictable.load(Ordering::Relaxed) {
                    PageFlags::PG_LRU | PageFlags::PG_UNEVICTABLE
                } else {
                    PageFlags::PG_LRU
                }
            };

            let page = page_manager_guard.create_one_page(
                PageType::File(FileMapInfo {
                    page_cache: self.page_cache_ref.clone(),
                    index: page_index,
                }),
                page_flags,
                &mut LockedFrameAllocator,
            )?;

            let page_len = core::cmp::min(MMArch::PAGE_SIZE, buf.len() - buf_offset);

            let mut page_guard = page.write_irqsave();
            unsafe {
                let dst = page_guard.as_slice_mut();
                dst[..page_len].copy_from_slice(&buf[buf_offset..buf_offset + page_len]);
            }

            self.add_page(start_page_index + i, &page);
        }

        Ok(())
    }

    /// 创建若干个"零页"并加入 PageCache。
    ///
    /// 与 `create_pages()` 的区别：
    /// - 不需要临时分配 `Vec<u8>` 作为填充缓冲区；
    /// - 直接分配物理页后在页内 `fill(0)`；
    ///
    /// 适用场景：tmpfs 等内存文件系统的"空洞读/缺页补零"。
    pub fn create_zero_pages(
        &mut self,
        start_page_index: usize,
        page_num: usize,
    ) -> Result<(), SystemError> {
        if page_num == 0 {
            return Ok(());
        }

        let mut page_manager_guard = page_manager_lock_irqsave();

        for i in 0..page_num {
            let page_index = start_page_index + i;

            let page_flags = {
                let cache = self
                    .page_cache_ref
                    .upgrade()
                    .expect("failed to get self_arc of pagecache");
                if cache.unevictable.load(Ordering::Relaxed) {
                    PageFlags::PG_LRU | PageFlags::PG_UNEVICTABLE
                } else {
                    PageFlags::PG_LRU
                }
            };

            let page = page_manager_guard.create_one_page(
                PageType::File(FileMapInfo {
                    page_cache: self.page_cache_ref.clone(),
                    index: page_index,
                }),
                page_flags,
                &mut LockedFrameAllocator,
            )?;

            let mut page_guard = page.write_irqsave();
            unsafe {
                page_guard.as_slice_mut().fill(0);
            }

            self.add_page(page_index, &page);
        }

        Ok(())
    }

    /// 创建若干个未初始化页面并返回引用，用于异步 IO。
    ///
    /// 与 `create_zero_pages()` 的区别：
    /// - 返回创建的 Arc<Page> 列表
    /// - 不预填充零或任何数据
    /// - 用于 BioVec 异步读取，数据由 IO 填充
    ///
    /// # 返回值
    /// - `Ok(Vec<Arc<Page>>)` 创建的页面列表
    /// - `Err(SystemError)` 创建失败
    fn create_pages_for_io(
        &mut self,
        start_page_index: usize,
        page_num: usize,
    ) -> Result<Vec<Arc<Page>>, SystemError> {
        if page_num == 0 {
            return Ok(Vec::new());
        }

        let mut page_manager_guard = page_manager_lock_irqsave();
        let mut pages = Vec::new();

        for i in 0..page_num {
            let page_index = start_page_index + i;

            let page_flags = {
                let cache = self
                    .page_cache_ref
                    .upgrade()
                    .expect("failed to get self_arc of pagecache");
                if cache.unevictable.load(Ordering::Relaxed) {
                    PageFlags::PG_LRU | PageFlags::PG_UNEVICTABLE
                } else {
                    PageFlags::PG_LRU
                }
            };

            let page = page_manager_guard.create_one_page(
                PageType::File(FileMapInfo {
                    page_cache: self.page_cache_ref.clone(),
                    index: page_index,
                }),
                page_flags,
                &mut LockedFrameAllocator,
            )?;

            self.add_page(page_index, &page);
            pages.push(page);
        }

        Ok(pages)
    }

    /// 从PageCache中读取数据（异步 BioVec 版本）。
    ///
    /// ## 参数
    ///
    /// - `offset` 偏移量
    /// - `buf_len` 缓冲区长度
    ///
    /// ## 返回值
    ///
    /// - `Ok((copies, ret, waiters))`
    ///   - `copies`: 已有页面或读取完成的页面的拷贝项
    ///   - `ret`: 成功读取的长度
    ///   - `waiters`: 待完成的异步 Bio 列表（调用者应在拷贝数据前等待）
    /// - `Err(SystemError)` 失败返回错误码
    fn prepare_read(
        &mut self,
        offset: usize,
        buf_len: usize,
    ) -> Result<(Vec<CopyItem>, usize, Vec<Arc<Bio>>), SystemError> {
        let inode: Arc<dyn IndexNode> = self
            .page_cache_ref
            .upgrade()
            .unwrap()
            .inode
            .upgrade()
            .unwrap();

        let file_size = inode.metadata().unwrap().size;

        // 尝试获取底层块设备用于异步 IO
        let block_device = inode.fs().get_block_device().ok();
        
        let len = if offset < file_size as usize {
            core::cmp::min(file_size as usize, offset + buf_len) - offset
        } else {
            0
        };

        if len == 0 {
            return Ok((Vec::new(), 0, Vec::new()));
        }

        let mut not_exist = Vec::new();
        let mut copies: Vec<CopyItem> = Vec::new();
        let mut waiters: Vec<Arc<Bio>> = Vec::new();

        let start_page_index = offset >> MMArch::PAGE_SHIFT;
        let page_num = (page_align_up(offset + len) >> MMArch::PAGE_SHIFT) - start_page_index;

        let mut ret = 0;
        for i in 0..page_num {
            let page_index = start_page_index + i;

            // 第一个页可能需要计算页内偏移
            let page_offset = if i == 0 {
                offset % MMArch::PAGE_SIZE
            } else {
                0
            };

            // 第一个页和最后一个页可能不满
            let sub_len = if i == 0 {
                min(len, MMArch::PAGE_SIZE - page_offset)
            } else if i == page_num - 1 {
                (offset + len - 1) % MMArch::PAGE_SIZE + 1
            } else {
                MMArch::PAGE_SIZE
            };

            if let Some(page) = self.get_page(page_index) {
                copies.push(CopyItem {
                    page,
                    page_offset,
                    sub_len,
                });
                ret += sub_len;
            } else if let Some((index, count)) = not_exist.last_mut() {
                if *index + *count == page_index {
                    *count += 1;
                } else {
                    not_exist.push((page_index, 1));
                }
            } else {
                not_exist.push((page_index, 1));
            }
        }

        // 读取不存在于缓存中的页面
        // 【重要】：对于文件系统，必须使用 inode.read_sync() 而不是直接提交 Bio
        // 原因：文件页偏移 (page_index) 不等于磁盘 LBA
        // 文件系统（如 ext4）使用 extent tree 将文件偏移映射到磁盘上分散的位置
        // inode.read_sync() 内部会正确处理这个映射，最终也会使用 Bio 机制
        //
        // 直接 Bio 提交（带 BioVec 零拷贝）仅适用于：
        // 1. 原始块设备访问（已知正确的 LBA）
        // 2. 文件系统提供了 file_offset_to_lba() 映射方法的情况
        //
        // TODO: 未来可以在 IndexNode trait 添加 submit_bio_for_page() 方法，
        //       让文件系统实现文件偏移到 LBA 的映射，以支持真正的零拷贝 DMA
        let _ = block_device; // 标记为已使用，避免警告

        for (page_index, count) in not_exist {
            // 创建未初始化页面
            let pages = self.create_pages_for_io(page_index, count)?;

            // 使用 inode.read_sync() 读取数据，它会正确处理文件偏移到磁盘 LBA 的映射
            let mut page_buf = vec![0u8; MMArch::PAGE_SIZE * count];
            inode.read_sync(page_index * MMArch::PAGE_SIZE, page_buf.as_mut())?;

            // 将数据写入已创建的页面
            for (i, page) in pages.iter().enumerate() {
                let buf_offset = i * MMArch::PAGE_SIZE;
                let page_len = core::cmp::min(MMArch::PAGE_SIZE, page_buf.len() - buf_offset);
                let mut page_guard = page.write_irqsave();
                unsafe {
                    let dst = page_guard.as_slice_mut();
                    dst[..page_len].copy_from_slice(&page_buf[buf_offset..buf_offset + page_len]);
                }
            }

            // 为每个新建的页生成拷贝项
            let copy_offset = core::cmp::max(page_index * MMArch::PAGE_SIZE, offset);
            let copy_len = core::cmp::min(
                (page_index + count) * MMArch::PAGE_SIZE,
                offset + len,
            ) - copy_offset;

            for (i, page) in pages.iter().enumerate() {
                let pg_index = page_index + i;
                let page_start = pg_index * MMArch::PAGE_SIZE;
                let sub_start = core::cmp::max(copy_offset, page_start);
                let sub_end =
                    core::cmp::min(copy_offset + copy_len, page_start + MMArch::PAGE_SIZE);
                if sub_end > sub_start {
                    copies.push(CopyItem {
                        page: page.clone(),
                        page_offset: sub_start - page_start,
                        sub_len: sub_end - sub_start,
                    });
                    ret += sub_end - sub_start;
                }
            }
        }

        Ok((copies, ret, waiters))
    }

    /// 向PageCache中写入数据。
    ///
    /// ## 参数
    ///
    /// - `offset` 偏移量
    /// - `buf` 缓冲区
    ///
    /// ## 返回值
    ///
    /// - `Ok(usize)` 成功读取的长度
    /// - `Err(SystemError)` 失败返回错误码
    pub fn write(
        &mut self,
        offset: usize,
        buf: &[u8],
    ) -> Result<(Vec<CopyItem>, usize), SystemError> {
        let len = buf.len();
        if len == 0 {
            return Ok((Vec::new(), 0));
        }

        let start_page_index = offset >> MMArch::PAGE_SHIFT;
        let page_num = (page_align_up(offset + len) >> MMArch::PAGE_SHIFT) - start_page_index;

        let mut copies: Vec<CopyItem> = Vec::new();
        let mut ret = 0;

        for i in 0..page_num {
            let page_index = start_page_index + i;

            // 第一个页可能需要计算页内偏移
            let page_offset = if i == 0 {
                offset % MMArch::PAGE_SIZE
            } else {
                0
            };

            // 第一个页和最后一个页可能不满
            let sub_len = if i == 0 {
                min(len, MMArch::PAGE_SIZE - page_offset)
            } else if i == page_num - 1 {
                (offset + len - 1) % MMArch::PAGE_SIZE + 1
            } else {
                MMArch::PAGE_SIZE
            };

            let mut page = self.get_page(page_index);

            if page.is_none() {
                let page_buf = vec![0u8; MMArch::PAGE_SIZE];
                self.create_pages(page_index, &page_buf)?;
                page = self.get_page(page_index);
            }

            if let Some(page) = page {
                copies.push(CopyItem {
                    page,
                    page_offset,
                    sub_len,
                });
                ret += sub_len;
            } else {
                return Err(SystemError::EIO);
            };
        }

        Ok((copies, ret))
    }

    pub fn resize(&mut self, len: usize) -> Result<(), SystemError> {
        let page_num = page_align_up(len) / MMArch::PAGE_SIZE;

        let mut reclaimer = page_reclaimer_lock();
        for (_i, page) in self.pages.drain_filter(|index, _page| *index >= page_num) {
            let _ = reclaimer.remove_page(&page.phys_address());
        }

        if page_num > 0 {
            let last_page_index = page_num - 1;
            let last_len = len - last_page_index * MMArch::PAGE_SIZE;
            if let Some(page) = self.get_page(last_page_index) {
                unsafe {
                    page.write_irqsave().truncate(last_len);
                };
            }
            // 对于新文件，最后一页不存在是正常的，不需要返回错误
            // 只有当文件需要截断到更小的尺寸时，才需要处理最后一页
        }

        Ok(())
    }

    pub fn pages_count(&self) -> usize {
        return self.pages.len();
    }

    /// Synchronize the page cache with the storage device.
    pub fn sync(&mut self) -> Result<(), SystemError> {
        for page in self.pages.values() {
            let mut guard = page.write_irqsave();
            if guard.flags().contains(PageFlags::PG_DIRTY) {
                crate::mm::page::PageReclaimer::page_writeback(&mut guard, false);
            }
        }
        Ok(())
    }

    /// 写回指定范围的脏页
    pub fn writeback_range(
        &mut self,
        start_index: usize,
        end_index: usize,
    ) -> Result<(), SystemError> {
        for idx in start_index..=end_index {
            if let Some(page) = self.pages.get(&idx) {
                let mut guard = page.write_irqsave();
                if guard.flags().contains(PageFlags::PG_DIRTY) {
                    crate::mm::page::PageReclaimer::page_writeback(&mut guard, false);
                }
            }
        }
        Ok(())
    }

    /// 驱逐指定范围的干净页
    ///
    /// 只驱逐干净的、无外部引用的页
    pub fn invalidate_range(&mut self, start_index: usize, end_index: usize) -> usize {
        let mut evicted = 0;
        let mut page_reclaimer = page_reclaimer_lock();

        for idx in start_index..=end_index {
            if let Some(page) = self.pages.get(&idx) {
                let guard = page.read_irqsave();
                if guard.flags().contains(PageFlags::PG_DIRTY) {
                    continue;
                }
                drop(guard);

                // 3处引用：1. page_cache中 2. page_manager中 3. lru中
                if Arc::strong_count(page) <= 3 {
                    if let Some(removed) = self.pages.remove(&idx) {
                        let paddr = removed.phys_address();
                        page_manager_lock_irqsave().remove_page(&paddr);
                        let _ = page_reclaimer.remove_page(&paddr);
                        evicted += 1;
                    }
                }
            }
        }

        evicted
    }
}

impl Drop for InnerPageCache {
    fn drop(&mut self) {
        // log::debug!("page cache drop");
        let mut page_manager = page_manager_lock_irqsave();
        for page in self.pages.values() {
            page_manager.remove_page(&page.phys_address());
        }
    }
}

impl PageCache {
    pub fn new(inode: Option<Weak<dyn IndexNode>>) -> Arc<PageCache> {
        let id = PAGE_CACHE_ID.fetch_add(1, Ordering::SeqCst);
        Arc::new_cyclic(|weak| Self {
            id,
            inner: Mutex::new(InnerPageCache::new(weak.clone(), id)),
            inode: {
                let v: Lazy<Weak<dyn IndexNode>> = Lazy::new();
                if let Some(inode) = inode {
                    v.init(inode);
                }
                v
            },
            unevictable: AtomicBool::new(false),
        })
    }

    /// # 获取页缓存的ID
    #[inline]
    #[allow(unused)]
    pub fn id(&self) -> usize {
        self.id
    }

    pub fn inode(&self) -> Option<Weak<dyn IndexNode>> {
        self.inode.try_get().cloned()
    }

    pub fn set_inode(&self, inode: Weak<dyn IndexNode>) -> Result<(), SystemError> {
        if self.inode.initialized() {
            return Err(SystemError::EINVAL);
        }
        self.inode.init(inode);
        Ok(())
    }

    pub fn lock(&self) -> MutexGuard<'_, InnerPageCache> {
        self.inner.lock()
    }

    /// Mark this page cache as unevictable (or revert). When enabled, newly created
    /// pages will carry PG_UNEVICTABLE to keep the reclaimer from reclaiming them.
    pub fn set_unevictable(&self, unevictable: bool) {
        self.unevictable.store(unevictable, Ordering::Relaxed);
    }

    /// 两阶段读取：持锁收集拷贝项和异步 IO，解锁后等待 IO 完成再拷贝到目标缓冲区，避免用户缺页导致自锁
    pub fn read(&self, offset: usize, buf: &mut [u8]) -> Result<usize, SystemError> {
        let (copies, ret, waiters) = {
            let mut guard = self.lock();
            guard.prepare_read(offset, buf.len())?
        };

        // 等待所有异步 Block I/O 完成
        for bio in waiters {
            bio.wait()?;
        }

        let mut dst_offset = 0;
        for item in copies {
            // 先prefault，避免在持锁后触发缺页
            let byte = volatile_read!(buf[dst_offset]);
            volatile_write!(buf[dst_offset], byte);
            let page_guard = item.page.read_irqsave();
            unsafe {
                buf[dst_offset..dst_offset + item.sub_len].copy_from_slice(
                    &page_guard.as_slice()[item.page_offset..item.page_offset + item.sub_len],
                );
            }
            dst_offset += item.sub_len;
        }

        Ok(ret)
    }

    /// 两阶段写入：持锁收集目标页，解锁后按页写入，避免用户缺页时持有page cache锁
    pub fn write(&self, offset: usize, buf: &[u8]) -> Result<usize, SystemError> {
        let (copies, ret) = {
            let mut guard = self.lock();
            guard.write(offset, buf)?
        };

        let mut src_offset = 0;
        for item in copies {
            // 预触发用户缓冲区当前段，避免后续在持页锁时缺页
            let _ = volatile_read!(buf[src_offset]);
            let mut page_guard = item.page.write_irqsave();
            unsafe {
                page_guard.as_slice_mut()[item.page_offset..item.page_offset + item.sub_len]
                    .copy_from_slice(&buf[src_offset..src_offset + item.sub_len]);
            }
            page_guard.add_flags(PageFlags::PG_DIRTY);
            src_offset += item.sub_len;
        }

        Ok(ret)
    }
}
