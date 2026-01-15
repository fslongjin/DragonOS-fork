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
use crate::libs::mutex::MutexGuard;
use crate::mm::page::FileMapInfo;
use crate::{arch::mm::LockedFrameAllocator, libs::lazy_init::Lazy};
use crate::{
    arch::MMArch,
    libs::mutex::Mutex,
    mm::{
        page::{page_manager_lock, page_reclaimer_lock, Page, PageFlags},
        MemoryManagementArch,
    },
};
use crate::{libs::align::page_align_up, mm::page::PageType};
use crate::mm::page_wait::{lock_page, unlock_page, wait_on_page_locked};

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
    need_read: bool,
    prelocked: bool,
}

pub enum PageFetchResult {
    Ready(Arc<Page>),
    NeedIo(Arc<Page>),
    Wait(Arc<Page>),
}

impl InnerPageCache {
    pub fn new(page_cache_ref: Weak<PageCache>, id: usize) -> InnerPageCache {
        Self {
            id,
            pages: HashMap::new(),
            page_cache_ref,
        }
    }

    fn base_page_flags(&self) -> PageFlags {
        let cache = self
            .page_cache_ref
            .upgrade()
            .expect("failed to get self_arc of pagecache");
        if cache.unevictable.load(Ordering::Relaxed) {
            PageFlags::PG_LRU | PageFlags::PG_UNEVICTABLE
        } else {
            PageFlags::PG_LRU
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

        let mut page_manager_guard = page_manager_lock();

        for i in 0..page_num {
            let buf_offset = i * MMArch::PAGE_SIZE;
            let page_index = start_page_index + i;

            if self.pages.contains_key(&page_index) {
                continue;
            }

            let page_flags = self.base_page_flags();

            let page = page_manager_guard.create_one_page(
                PageType::File(FileMapInfo {
                    page_cache: self.page_cache_ref.clone(),
                    index: page_index,
                }),
                page_flags,
                &mut LockedFrameAllocator,
            )?;

            let page_len = core::cmp::min(MMArch::PAGE_SIZE, buf.len() - buf_offset);

            let mut page_guard = page.write();
            unsafe {
                let dst = page_guard.as_slice_mut();
                dst[..page_len].copy_from_slice(&buf[buf_offset..buf_offset + page_len]);
            }
            page_guard.add_flags(PageFlags::PG_UPTODATE);

            self.add_page(start_page_index + i, &page);
        }

        Ok(())
    }

    /// 创建若干个“零页”并加入 PageCache。
    ///
    /// 与 `create_pages()` 的区别：
    /// - 不需要临时分配 `Vec<u8>` 作为填充缓冲区；
    /// - 直接分配物理页后在页内 `fill(0)`；
    ///
    /// 适用场景：tmpfs 等内存文件系统的“空洞读/缺页补零”。
    pub fn create_zero_pages(
        &mut self,
        start_page_index: usize,
        page_num: usize,
    ) -> Result<(), SystemError> {
        if page_num == 0 {
            return Ok(());
        }

        let mut page_manager_guard = page_manager_lock();

        for i in 0..page_num {
            let page_index = start_page_index + i;

            if self.pages.contains_key(&page_index) {
                continue;
            }

            let page_flags = self.base_page_flags();

            let page = page_manager_guard.create_one_page(
                PageType::File(FileMapInfo {
                    page_cache: self.page_cache_ref.clone(),
                    index: page_index,
                }),
                page_flags,
                &mut LockedFrameAllocator,
            )?;

            let mut page_guard = page.write();
            unsafe {
                page_guard.as_slice_mut().fill(0);
            }
            page_guard.add_flags(PageFlags::PG_UPTODATE);

            self.add_page(page_index, &page);
        }

        Ok(())
    }

    pub fn create_empty_page(&mut self, page_index: usize) -> Result<Arc<Page>, SystemError> {
        let mut page_manager_guard = page_manager_lock();
        let page_flags = self.base_page_flags();
        let page = page_manager_guard.create_one_page(
            PageType::File(FileMapInfo {
                page_cache: self.page_cache_ref.clone(),
                index: page_index,
            }),
            page_flags,
            &mut LockedFrameAllocator,
        )?;
        self.add_page(page_index, &page);
        Ok(page)
    }

    pub fn get_or_create_locked_page(
        &mut self,
        page_index: usize,
    ) -> Result<PageFetchResult, SystemError> {
        if let Some(page) = self.get_page(page_index) {
            let flags = *page.read().flags();
            if flags.contains(PageFlags::PG_LOCKED) {
                page.write().add_flags(PageFlags::PG_WAITERS);
                return Ok(PageFetchResult::Wait(page));
            }
            if flags.contains(PageFlags::PG_UPTODATE) || flags.contains(PageFlags::PG_ERROR) {
                return Ok(PageFetchResult::Ready(page));
            }
            page.write().add_flags(PageFlags::PG_LOCKED);
            return Ok(PageFetchResult::NeedIo(page));
        }

        let page = self.create_empty_page(page_index)?;
        page.write().add_flags(PageFlags::PG_LOCKED);
        Ok(PageFetchResult::NeedIo(page))
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
            let mut prelocked = false;
            let need_read = page_offset != 0 || sub_len != MMArch::PAGE_SIZE;

            if page.is_none() {
                let new_page = self.create_empty_page(page_index)?;
                new_page.write().add_flags(PageFlags::PG_LOCKED);
                page = Some(new_page);
                prelocked = true;
            }

            if let Some(page) = page {
                copies.push(CopyItem {
                    page,
                    page_offset,
                    sub_len,
                    need_read,
                    prelocked,
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
                    page.write().truncate(last_len);
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
            let mut guard = page.write();
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
                let mut guard = page.write();
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
                let guard = page.read();
                if guard.flags().contains(PageFlags::PG_DIRTY) {
                    continue;
                }
                drop(guard);

                // 3处引用：1. page_cache中 2. page_manager中 3. lru中
                if Arc::strong_count(page) <= 3 {
                    if let Some(removed) = self.pages.remove(&idx) {
                        let paddr = removed.phys_address();
                        page_manager_lock().remove_page(&paddr);
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
        let mut page_manager = page_manager_lock();
        for page in self.pages.values() {
            page_manager.remove_page(&page.phys_address());
        }
    }
}

impl PageCache {
    fn load_read_page(
        &self,
        page_index: usize,
        inode: &Arc<dyn IndexNode>,
    ) -> Result<Arc<Page>, SystemError> {
        loop {
            let fetch = {
                let mut guard = self.inner.lock();
                guard.get_or_create_locked_page(page_index)?
            };

            match fetch {
                PageFetchResult::Ready(page) => {
                    if page.read().flags().contains(PageFlags::PG_ERROR) {
                        return Err(SystemError::EIO);
                    }
                    return Ok(page);
                }
                PageFetchResult::Wait(page) => {
                    wait_on_page_locked(&page, false)?;
                    if page.read().flags().contains(PageFlags::PG_ERROR) {
                        return Err(SystemError::EIO);
                    }
                    continue;
                }
                PageFetchResult::NeedIo(page) => {
                    let mut page_guard = page.write();
                    let page_buf = unsafe { page_guard.as_slice_mut() };
                    let mut filled = 0;
                    let mut io_err = None;
                    while filled < MMArch::PAGE_SIZE {
                        match inode.read_sync(
                            page_index * MMArch::PAGE_SIZE + filled,
                            &mut page_buf[filled..],
                        ) {
                            Ok(0) => break,
                            Ok(read_len) => {
                                filled += read_len;
                            }
                            Err(e) => {
                                io_err = Some(e);
                                break;
                            }
                        }
                    }
                    if let Some(e) = io_err {
                        page_guard.add_flags(PageFlags::PG_ERROR);
                        drop(page_guard);
                        unlock_page(&page);
                        return Err(e);
                    }
                    if filled == 0 {
                        page_guard.add_flags(PageFlags::PG_ERROR);
                        drop(page_guard);
                        unlock_page(&page);
                        return Err(SystemError::EIO);
                    }
                    if filled < MMArch::PAGE_SIZE {
                        page_buf[filled..].fill(0);
                    }
                    page_guard.remove_flags(PageFlags::PG_ERROR);
                    page_guard.add_flags(PageFlags::PG_UPTODATE);
                    drop(page_guard);
                    unlock_page(&page);
                    return Ok(page);
                }
            }
        }
    }

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

    /// 两阶段读取：持锁收集拷贝项，解锁后拷贝到目标缓冲区，避免用户缺页导致自锁
    pub fn read(&self, offset: usize, buf: &mut [u8]) -> Result<usize, SystemError> {
        let inode: Arc<dyn IndexNode> = self
            .inode
            .try_get()
            .and_then(|inode| inode.upgrade())
            .ok_or(SystemError::EIO)?;

        let file_size = inode.metadata()?.size.max(0) as usize;
        let len = if offset < file_size {
            core::cmp::min(file_size, offset + buf.len()) - offset
        } else {
            0
        };

        if len == 0 {
            return Ok(0);
        }

        let start_page_index = offset >> MMArch::PAGE_SHIFT;
        let page_num = (page_align_up(offset + len) >> MMArch::PAGE_SHIFT) - start_page_index;

        let mut copies: Vec<CopyItem> = Vec::new();
        let mut ret = 0;
        for i in 0..page_num {
            let page_index = start_page_index + i;
            let page_offset = if i == 0 {
                offset % MMArch::PAGE_SIZE
            } else {
                0
            };
            let sub_len = if i == 0 {
                min(len, MMArch::PAGE_SIZE - page_offset)
            } else if i == page_num - 1 {
                (offset + len - 1) % MMArch::PAGE_SIZE + 1
            } else {
                MMArch::PAGE_SIZE
            };

            let page = self.load_read_page(page_index, &inode)?;
            copies.push(CopyItem {
                page,
                page_offset,
                sub_len,
                need_read: false,
                prelocked: false,
            });
            ret += sub_len;
        }

        let mut dst_offset = 0;
        for item in copies {
            // 先prefault，避免在持锁后触发缺页
            let byte = volatile_read!(buf[dst_offset]);
            volatile_write!(buf[dst_offset], byte);
            let page_guard = item.page.read();
            if page_guard.flags().contains(PageFlags::PG_ERROR) {
                return Err(SystemError::EIO);
            }
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
            let mut guard = self.inner.lock();
            guard.write(offset, buf)?
        };

        let inode = self
            .inode
            .try_get()
            .and_then(|inode| inode.upgrade());

        let mut src_offset = 0;
        for item in copies {
            // 预触发用户缓冲区当前段，避免后续在持页锁时缺页
            let _ = volatile_read!(buf[src_offset]);
            if !item.prelocked {
                lock_page(&item.page, false)?;
            }
            let mut page_guard = item.page.write();
            if item.need_read && !page_guard.flags().contains(PageFlags::PG_UPTODATE) {
                let inode = inode.as_ref().ok_or(SystemError::EIO)?;
                let page_buf = unsafe { page_guard.as_slice_mut() };
                let page_start = (offset + src_offset - item.page_offset) & !(MMArch::PAGE_SIZE - 1);
                let mut filled = 0;
                let mut io_err = None;
                while filled < MMArch::PAGE_SIZE {
                    match inode.read_sync(page_start + filled, &mut page_buf[filled..]) {
                        Ok(0) => break,
                        Ok(read_len) => {
                            filled += read_len;
                        }
                        Err(e) => {
                            io_err = Some(e);
                            break;
                        }
                    }
                }
                if let Some(e) = io_err {
                    page_guard.add_flags(PageFlags::PG_ERROR);
                    drop(page_guard);
                    unlock_page(&item.page);
                    return Err(e);
                }
                if filled < MMArch::PAGE_SIZE {
                    page_buf[filled..].fill(0);
                }
                if filled == 0 {
                    page_guard.add_flags(PageFlags::PG_ERROR);
                    drop(page_guard);
                    unlock_page(&item.page);
                    return Err(SystemError::EIO);
                }
            }
            unsafe {
                page_guard.as_slice_mut()[item.page_offset..item.page_offset + item.sub_len]
                    .copy_from_slice(&buf[src_offset..src_offset + item.sub_len]);
            }
            page_guard.add_flags(PageFlags::PG_DIRTY | PageFlags::PG_UPTODATE);
            drop(page_guard);
            unlock_page(&item.page);
            src_offset += item.sub_len;
        }

        Ok(ret)
    }
}
