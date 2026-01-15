## PageCache 并发缺页修复 TODO

- [x] 引入全局 PageWaitTable（固定桶）并提供 `lock_page/wait_on_page_locked/unlock_page` 原语
- [x] 为 PageCache 创建页时补齐 `PG_UPTODATE` 标记（create_pages/create_zero_pages/write）
- [x] 在 PageCache 中新增 `get_or_create_locked_page` 以区分 Ready/NeedIo/Wait
- [x] filemap_fault 走 `read_sync`，并按页锁语义串行 I/O + 等待
- [x] 审核写回/回收路径是否需要在 `PG_LOCKED` 下操作（避免与读缺页冲突）
- [ ] 补充页错误路径测试（SIGBUS、短读、IO 错误）
- [ ] 性能回归评估：高并发 mmap 读场景
