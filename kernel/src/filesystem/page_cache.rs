//! 页缓存 (PageCache) 模块
//!
//! 本模块实现了文件系统的页缓存机制，包括：
//! - LRU缓存管理
//! - 页状态管理 (UpToDate/Dirty)
//! - 预读机制 (Readahead)
//! - PageCacheBackend trait 用于后端存储抽象
//!
//! # 设计参考
//! - Asterinas 的 PageCache 设计
//! - Linux 内核的 page cache 机制

use core::{
    cmp::min,
    ops::Range,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};
use hashbrown::HashMap;
use lru::LruCache;
use system_error::SystemError;

use super::vfs::IndexNode;
use crate::libs::spinlock::SpinLockGuard;
use crate::mm::page::FileMapInfo;
use crate::{arch::mm::LockedFrameAllocator, libs::lazy_init::Lazy};
use crate::{
    arch::MMArch,
    libs::spinlock::SpinLock,
    mm::{
        page::{page_manager_lock_irqsave, page_reclaimer_lock_irqsave, Page, PageFlags},
        MemoryManagementArch,
    },
};
use crate::{libs::align::page_align_up, mm::page::PageType};

// ============================================================================
// 常量定义
// ============================================================================

/// 默认最大缓存页数 (0 表示不限制)
pub const DEFAULT_MAX_CACHE_PAGES: usize = 0;

/// 预读初始窗口大小 (页数)
pub const READAHEAD_INIT_SIZE: usize = 4;

/// 预读最大窗口大小 (页数)
pub const READAHEAD_MAX_SIZE: usize = 32;

// ============================================================================
// PageCache 主结构
// ============================================================================

static PAGE_CACHE_ID: AtomicUsize = AtomicUsize::new(0);

/// 页面缓存
///
/// 管理文件的页缓存，提供读写接口，支持LRU淘汰和预读优化
#[derive(Debug)]
pub struct PageCache {
    id: usize,
    inner: SpinLock<InnerPageCache>,
    inode: Lazy<Weak<dyn IndexNode>>,
    unevictable: AtomicBool,
}

/// PageCache 内部结构
#[derive(Debug)]
pub struct InnerPageCache {
    #[allow(unused)]
    id: usize,
    /// 使用 HashMap 存储页面
    /// key: 页索引, value: 页面
    pages: HashMap<usize, Arc<Page>>,
    /// LRU 访问顺序记录
    lru_order: LruCache<usize, ()>,
    /// 最大缓存页数 (0 表示不限制)
    max_pages: usize,
    /// 指向 PageCache 的弱引用
    page_cache_ref: Weak<PageCache>,
    /// 预读状态
    readahead: ReadaheadState,
}

/// 描述一次从页缓存到目标缓冲区的拷贝
pub struct CopyItem {
    page: Arc<Page>,
    page_offset: usize,
    sub_len: usize,
}

// ============================================================================
// 预读机制
// ============================================================================

/// 预读状态
#[derive(Debug)]
pub struct ReadaheadState {
    /// 当前预读窗口
    window: Option<ReadaheadWindow>,
    /// 最大预读窗口大小
    max_size: usize,
    /// 上次访问的页索引
    prev_page: Option<usize>,
}

/// 预读窗口
#[derive(Debug, Clone)]
pub struct ReadaheadWindow {
    /// 预读范围 [start, end)
    pub range: Range<usize>,
    /// 触发下次预读的页索引
    pub lookahead_index: usize,
}

impl ReadaheadState {
    /// 创建新的预读状态
    pub fn new() -> Self {
        ReadaheadState {
            window: None,
            max_size: READAHEAD_MAX_SIZE,
            prev_page: None,
        }
    }

    /// 检查是否应该触发预读
    ///
    /// # 参数
    /// - `page_index`: 当前访问的页索引
    /// - `max_page`: 文件最大页索引
    ///
    /// # 返回
    /// - `Some(ReadaheadWindow)`: 需要预读的窗口
    /// - `None`: 不需要预读
    pub fn should_readahead(
        &mut self,
        page_index: usize,
        max_page: usize,
    ) -> Option<ReadaheadWindow> {
        // 检查是否是顺序访问
        let is_sequential = match self.prev_page {
            Some(prev) => page_index == prev + 1 || page_index == prev,
            None => true,
        };

        self.prev_page = Some(page_index);

        if !is_sequential {
            // 非顺序访问，重置预读状态
            self.window = None;
            return None;
        }

        match &self.window {
            None => {
                // 首次顺序访问，初始化预读窗口
                let size = min(READAHEAD_INIT_SIZE, max_page.saturating_sub(page_index));
                if size == 0 {
                    return None;
                }
                let start = page_index;
                let end = start + size;
                let lookahead = start + size / 2;

                let window = ReadaheadWindow {
                    range: start..end,
                    lookahead_index: lookahead,
                };
                self.window = Some(window.clone());
                Some(window)
            }
            Some(window) => {
                // 检查是否命中 lookahead 点
                if page_index >= window.lookahead_index && page_index < window.range.end {
                    // 扩展预读窗口
                    let new_start = window.range.end;
                    let new_size = min(
                        min((window.range.end - window.range.start) * 2, self.max_size),
                        max_page.saturating_sub(new_start),
                    );
                    if new_size == 0 {
                        return None;
                    }
                    let new_end = new_start + new_size;
                    let new_lookahead = new_start + new_size / 2;

                    let new_window = ReadaheadWindow {
                        range: new_start..new_end,
                        lookahead_index: new_lookahead,
                    };
                    self.window = Some(new_window.clone());
                    Some(new_window)
                } else {
                    None
                }
            }
        }
    }

    /// 重置预读状态
    pub fn reset(&mut self) {
        self.window = None;
        self.prev_page = None;
    }
}

impl Default for ReadaheadState {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// PageCacheBackend trait
// ============================================================================

/// 页缓存后端 trait
///
/// 文件系统需要实现此 trait 来支持 PageCache 的读写操作
pub trait PageCacheBackend: Send + Sync {
    /// 同步读取页面数据
    ///
    /// # 参数
    /// - `page_index`: 页索引
    /// - `buf`: 输出缓冲区 (大小应为 PAGE_SIZE)
    ///
    /// # 返回
    /// - `Ok(usize)`: 读取的字节数
    /// - `Err(SystemError)`: 错误
    fn read_page(&self, page_index: usize, buf: &mut [u8]) -> Result<usize, SystemError>;

    /// 同步写入页面数据
    ///
    /// # 参数
    /// - `page_index`: 页索引
    /// - `buf`: 输入缓冲区 (大小应为 PAGE_SIZE)
    ///
    /// # 返回
    /// - `Ok(usize)`: 写入的字节数
    /// - `Err(SystemError)`: 错误
    fn write_page(&self, page_index: usize, buf: &[u8]) -> Result<usize, SystemError>;

    /// 获取文件的总页数
    fn npages(&self) -> usize;
}

// ============================================================================
// IndexNode 的 PageCacheBackend 实现
// ============================================================================

/// IndexNode 的 PageCacheBackend 包装器
///
/// 通过 IndexNode::read_sync/write_sync 实现 PageCacheBackend
pub struct InodeBackend {
    inode: Arc<dyn IndexNode>,
}

impl InodeBackend {
    /// 创建新的 InodeBackend
    pub fn new(inode: Arc<dyn IndexNode>) -> Self {
        Self { inode }
    }
}

impl PageCacheBackend for InodeBackend {
    fn read_page(&self, page_index: usize, buf: &mut [u8]) -> Result<usize, SystemError> {
        let offset = page_index * MMArch::PAGE_SIZE;
        self.inode.read_sync(offset, buf)
    }

    fn write_page(&self, page_index: usize, buf: &[u8]) -> Result<usize, SystemError> {
        let offset = page_index * MMArch::PAGE_SIZE;
        self.inode.write_sync(offset, buf)
    }

    fn npages(&self) -> usize {
        if let Ok(meta) = self.inode.metadata() {
            let size = meta.size as usize;
            (size + MMArch::PAGE_SIZE - 1) / MMArch::PAGE_SIZE
        } else {
            0
        }
    }
}

// ============================================================================
// InnerPageCache 实现
// ============================================================================

impl InnerPageCache {
    pub fn new(page_cache_ref: Weak<PageCache>, id: usize) -> InnerPageCache {
        Self {
            id,
            pages: HashMap::new(),
            lru_order: LruCache::unbounded(),
            max_pages: DEFAULT_MAX_CACHE_PAGES,
            page_cache_ref,
            readahead: ReadaheadState::new(),
        }
    }

    /// 设置最大缓存页数
    pub fn set_max_pages(&mut self, max_pages: usize) {
        self.max_pages = max_pages;
    }

    /// 添加页面到缓存
    pub fn add_page(&mut self, offset: usize, page: &Arc<Page>) {
        self.pages.insert(offset, page.clone());
        self.lru_order.put(offset, ());

        // 如果设置了最大页数限制，尝试淘汰
        if self.max_pages > 0 {
            self.try_shrink();
        }
    }

    /// 获取页面 (会更新LRU顺序)
    pub fn get_page(&mut self, offset: usize) -> Option<Arc<Page>> {
        if let Some(page) = self.pages.get(&offset) {
            // 更新 LRU 顺序
            self.lru_order.get(&offset);
            Some(page.clone())
        } else {
            None
        }
    }

    /// 获取页面 (不更新LRU顺序，用于只读查询)
    pub fn peek_page(&self, offset: usize) -> Option<Arc<Page>> {
        self.pages.get(&offset).cloned()
    }

    /// 移除页面
    pub fn remove_page(&mut self, offset: usize) -> Option<Arc<Page>> {
        self.lru_order.pop(&offset);
        self.pages.remove(&offset)
    }

    /// 尝试收缩缓存
    ///
    /// 当缓存页数超过最大限制时，淘汰最久未使用的干净页
    fn try_shrink(&mut self) {
        if self.max_pages == 0 || self.pages.len() <= self.max_pages {
            return;
        }

        let mut to_evict = Vec::new();
        let target = self.pages.len() - self.max_pages;

        // 从 LRU 尾部开始查找可淘汰的页
        for (idx, _) in self.lru_order.iter().rev() {
            if to_evict.len() >= target {
                break;
            }
            if let Some(page) = self.pages.get(idx) {
                let guard = page.read_irqsave();
                // 只淘汰干净页
                if !guard.flags().contains(PageFlags::PG_DIRTY) {
                    to_evict.push(*idx);
                }
            }
        }

        // 执行淘汰
        let mut page_reclaimer = page_reclaimer_lock_irqsave();
        for idx in to_evict {
            if let Some(page) = self.pages.remove(&idx) {
                self.lru_order.pop(&idx);
                let paddr = page.phys_address();
                page_manager_lock_irqsave().remove_page(&paddr);
                let _ = page_reclaimer.remove_page(&paddr);
            }
        }
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
                    PageFlags::PG_LRU | PageFlags::PG_UNEVICTABLE | PageFlags::PG_UPTODATE
                } else {
                    PageFlags::PG_LRU | PageFlags::PG_UPTODATE
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

            drop(page_guard);
            drop(page_manager_guard);

            self.add_page(page_index, &page);

            page_manager_guard = page_manager_lock_irqsave();
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
                    PageFlags::PG_LRU | PageFlags::PG_UNEVICTABLE | PageFlags::PG_UPTODATE
                } else {
                    PageFlags::PG_LRU | PageFlags::PG_UPTODATE
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

            drop(page_guard);
            drop(page_manager_guard);

            self.add_page(page_index, &page);

            page_manager_guard = page_manager_lock_irqsave();
        }

        Ok(())
    }

    /// 执行预读操作
    ///
    /// # 参数
    /// - `inode`: 文件inode
    /// - `window`: 预读窗口
    fn do_readahead(
        &mut self,
        inode: &Arc<dyn IndexNode>,
        window: &ReadaheadWindow,
    ) -> Result<(), SystemError> {
        for page_index in window.range.clone() {
            // 跳过已存在的页
            if self.peek_page(page_index).is_some() {
                continue;
            }

            // 读取页面数据
            let mut page_buf = vec![0u8; MMArch::PAGE_SIZE];
            if inode
                .read_sync(page_index * MMArch::PAGE_SIZE, &mut page_buf)
                .is_err()
            {
                // 预读失败不影响主流程
                break;
            }

            // 创建页面并标记为预读
            self.create_pages(page_index, &page_buf)?;

            // 标记预读页
            if let Some(page) = self.peek_page(page_index) {
                page.write_irqsave().add_flags(PageFlags::PG_READAHEAD);
            }
        }
        Ok(())
    }

    /// 从PageCache中读取数据。
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
    fn prepare_read(
        &mut self,
        offset: usize,
        buf_len: usize,
    ) -> Result<(Vec<CopyItem>, usize), SystemError> {
        let inode: Arc<dyn IndexNode> = self
            .page_cache_ref
            .upgrade()
            .unwrap()
            .inode
            .upgrade()
            .unwrap();

        let file_size = inode.metadata().unwrap().size;

        let len = if offset < file_size as usize {
            core::cmp::min(file_size as usize, offset + buf_len) - offset
        } else {
            0
        };

        if len == 0 {
            return Ok((Vec::new(), 0));
        }

        let start_page_index = offset >> MMArch::PAGE_SHIFT;
        let page_num = (page_align_up(offset + len) >> MMArch::PAGE_SHIFT) - start_page_index;
        let max_page = (file_size as usize + MMArch::PAGE_SIZE - 1) >> MMArch::PAGE_SHIFT;

        // 检查预读
        if let Some(window) = self.readahead.should_readahead(start_page_index, max_page) {
            let _ = self.do_readahead(&inode, &window);
        }

        let mut not_exist = Vec::new();
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

        for (page_index, count) in not_exist {
            // TODO 这里使用buffer避免多次读取磁盘，将来引入异步IO直接写入页面，减少内存开销和拷贝
            let mut page_buf = vec![0u8; MMArch::PAGE_SIZE * count];

            inode.read_sync(page_index * MMArch::PAGE_SIZE, page_buf.as_mut())?;

            self.create_pages(page_index, page_buf.as_mut())?;

            // 实际要拷贝的内容在文件中的偏移量
            let copy_offset = core::cmp::max(page_index * MMArch::PAGE_SIZE, offset);
            // 实际要拷贝的内容的长度
            let copy_len = core::cmp::min((page_index + count) * MMArch::PAGE_SIZE, offset + len)
                - copy_offset;

            // 为每个新建的页生成拷贝项
            for i in 0..count {
                let pg_index = page_index + i;
                let page = self
                    .get_page(pg_index)
                    .expect("page must exist after create_pages");
                let page_start = pg_index * MMArch::PAGE_SIZE;
                let sub_start = core::cmp::max(copy_offset, page_start);
                let sub_end =
                    core::cmp::min(copy_offset + copy_len, page_start + MMArch::PAGE_SIZE);
                if sub_end > sub_start {
                    copies.push(CopyItem {
                        page,
                        page_offset: sub_start - page_start,
                        sub_len: sub_end - sub_start,
                    });
                    ret += sub_end - sub_start;
                }
            }
        }

        Ok((copies, ret))
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

        let mut reclaimer = page_reclaimer_lock_irqsave();

        // 收集要删除的页索引
        let to_remove: Vec<usize> = self
            .pages
            .keys()
            .filter(|&&idx| idx >= page_num)
            .cloned()
            .collect();

        for idx in to_remove {
            if let Some(page) = self.pages.remove(&idx) {
                self.lru_order.pop(&idx);
                let _ = reclaimer.remove_page(&page.phys_address());
            }
        }

        if page_num > 0 {
            let last_page_index = page_num - 1;
            let last_len = len - last_page_index * MMArch::PAGE_SIZE;
            if let Some(page) = self.peek_page(last_page_index) {
                unsafe {
                    page.write_irqsave().truncate(last_len);
                };
            }
            // 对于新文件，最后一页不存在是正常的，不需要返回错误
            // 只有当文件需要截断到更小的尺寸时，才需要处理最后一页
        }

        // 重置预读状态
        self.readahead.reset();

        Ok(())
    }

    pub fn pages_count(&self) -> usize {
        self.pages.len()
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

    /// 批量写回脏页
    ///
    /// 优化版本：收集连续的脏页范围，尝试合并 IO 操作
    ///
    /// # 参数
    /// - `max_pages`: 最多写回的页数，0 表示全部
    ///
    /// # 返回
    /// - `Ok(usize)`: 写回的页数
    pub fn writeback_batch(&mut self, max_pages: usize) -> Result<usize, SystemError> {
        let max_pages = if max_pages == 0 {
            self.pages.len()
        } else {
            max_pages
        };

        // 收集脏页索引
        let mut dirty_indices: Vec<usize> = self
            .pages
            .iter()
            .filter_map(|(&idx, page)| {
                let guard = page.read_irqsave();
                if guard.flags().contains(PageFlags::PG_DIRTY) {
                    Some(idx)
                } else {
                    None
                }
            })
            .take(max_pages)
            .collect();

        // 按索引排序以优化 IO 顺序
        dirty_indices.sort_unstable();

        // 合并连续范围
        let ranges = Self::merge_page_ranges(&dirty_indices);

        let mut written = 0;
        for (start, count) in ranges {
            for idx in start..start + count {
                if let Some(page) = self.pages.get(&idx) {
                    let mut guard = page.write_irqsave();
                    if guard.flags().contains(PageFlags::PG_DIRTY) {
                        crate::mm::page::PageReclaimer::page_writeback(&mut guard, false);
                        written += 1;
                    }
                }
            }
        }

        Ok(written)
    }

    /// 合并连续页索引为范围
    ///
    /// 例如: [1, 2, 3, 5, 6, 8] -> [(1, 3), (5, 2), (8, 1)]
    fn merge_page_ranges(indices: &[usize]) -> Vec<(usize, usize)> {
        if indices.is_empty() {
            return Vec::new();
        }

        let mut ranges = Vec::new();
        let mut start = indices[0];
        let mut count = 1;

        for &idx in &indices[1..] {
            if idx == start + count {
                // 连续
                count += 1;
            } else {
                // 不连续，保存当前范围，开始新范围
                ranges.push((start, count));
                start = idx;
                count = 1;
            }
        }

        // 保存最后一个范围
        ranges.push((start, count));
        ranges
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
        let mut page_reclaimer = page_reclaimer_lock_irqsave();

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
                        self.lru_order.pop(&idx);
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

// ============================================================================
// PageCache 实现
// ============================================================================

impl PageCache {
    pub fn new(inode: Option<Weak<dyn IndexNode>>) -> Arc<PageCache> {
        let id = PAGE_CACHE_ID.fetch_add(1, Ordering::SeqCst);
        Arc::new_cyclic(|weak| Self {
            id,
            inner: SpinLock::new(InnerPageCache::new(weak.clone(), id)),
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

    pub fn lock_irqsave(&self) -> SpinLockGuard<'_, InnerPageCache> {
        if self.inner.is_locked() {
            log::error!("page cache already locked");
        }
        self.inner.lock_irqsave()
    }

    pub fn is_locked(&self) -> bool {
        self.inner.is_locked()
    }

    /// Mark this page cache as unevictable (or revert). When enabled, newly created
    /// pages will carry PG_UNEVICTABLE to keep the reclaimer from reclaiming them.
    pub fn set_unevictable(&self, unevictable: bool) {
        self.unevictable.store(unevictable, Ordering::Relaxed);
    }

    /// 设置最大缓存页数
    ///
    /// # 参数
    /// - `max_pages`: 最大页数，0表示不限制
    pub fn set_max_pages(&self, max_pages: usize) {
        self.inner.lock_irqsave().set_max_pages(max_pages);
    }

    /// 两阶段读取：持锁收集拷贝项，解锁后拷贝到目标缓冲区，避免用户缺页导致自锁
    pub fn read(&self, offset: usize, buf: &mut [u8]) -> Result<usize, SystemError> {
        let (copies, ret) = {
            let mut guard = self.inner.lock_irqsave();
            guard.prepare_read(offset, buf.len())?
        };

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
            let mut guard = self.inner.lock_irqsave();
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
