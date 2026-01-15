## PageCache 并发窗口复查清单

### 1) filemap_fault

- [x] 缺页路径使用 `get_or_create_locked_page`，确保同页只有一个线程执行 I/O
- [x] I/O 直接写入目标页，完成后设置 `PG_UPTODATE/PG_ERROR` 并 `unlock_page`
- [x] 等待路径使用 `wait_on_page_locked`，可处理中断并按需返回 `VM_FAULT_RETRY`
- [x] 短读零填充，避免未初始化尾部被映射
- [ ] 风险确认：`read_sync` 的 inode 实现是否可能间接走 page cache 导致递归

涉及文件：`kernel/src/mm/fault.rs`

### 2) writeback / reclaim

- [x] 写回前对脏页 `lock_page`，写回后 `unlock_page`，避免与读缺页并发
- [x] 回收路径写回失败时释放锁并回插 LRU
- [ ] 风险确认：`page_writeback` 对 `vma_set` 的并发访问是否需要额外同步（目前假设 VMA 生命周期稳定）

涉及文件：`kernel/src/mm/page.rs`

### 3) PageCache read/write

- [x] `prepare_read` 逐页 `get_or_create_locked_page`，不再使用临时 buffer
- [x] `PG_ERROR` 显式转 `EIO`
- [x] `read` 侧等待 `PG_LOCKED`，避免读到半写页
- [x] `write` 对部分写先读旧页（若未 `UPTODATE`），再覆盖写，防止零覆盖
- [x] 新页写入阶段持 `PG_LOCKED`，避免写读并发窗口

涉及文件：`kernel/src/filesystem/page_cache.rs`

### 4) readahead

- [ ] `readahead` 读取路径是否使用 `get_or_create_locked_page` 或等效锁定（当前仍走 `create_pages`）
- [ ] `PG_READAHEAD` 与 `PG_LOCKED` 的配合是否完整（是否会提前映射未就绪页）
- [ ] 预读 I/O 错误处理是否设置 `PG_ERROR`

涉及文件：`kernel/src/mm/readahead.rs`

### 5) 其它潜在并发窗口

- [ ] `create_pages/create_zero_pages` 仅用于“无人持锁的批量创建场景”，并确保跳过已存在页
- [ ] `pagecache_fault_zero` 是否需要 `PG_LOCKED` 保护（tmpfs 多线程缺页）
- [ ] `filemap_map_pages` 仅映射 `PG_UPTODATE` 页，已符合

### 结论与下一步建议

- 建议补齐 readahead 路径的 “锁页 + UPTODATE + ERROR” 语义，以统一行为。
- 如需严格一致性，可将 `pagecache_fault_zero` 也改成 `get_or_create_locked_page` 流程。
