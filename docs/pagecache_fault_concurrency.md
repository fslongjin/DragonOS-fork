## 背景与问题定位

当前 DragonOS 的 `filemap_fault()` 在页缓存缺页时缺少“加载中/就绪”的并发状态管理，多个线程/进程并发触发同一页缺页会产生竞态。典型路径：

1. `filemap_fault()` 先 `page_cache.lock().get_page()`；若为空就调用 `file.pread()`。
2. `file.pread()` 走 `inode.read_at()` -> `PageCache::read()` -> `create_pages()` 把页插入缓存。
3. `filemap_fault()` 再次 `get_page()`，把页直接映射。

问题点：

- **重复创建/覆盖**：`create_pages()` 不检查“同页是否已存在”，在并发场景会把已有页覆盖掉，旧页仍留在 `page_manager/LRU`，导致资源泄露或回收异常。
- **无“加载中”状态**：如果线程 A 已创建页但尚未完成 I/O，线程 B 可能拿到该页并直接映射，导致映射到未填充/半完成页。
- **UPTODATE 语义缺失**：当前对文件页的创建流程未设置 `PG_UPTODATE`，`filemap_map_pages()` 又只映射 `PG_UPTODATE` 页；语义不一致会导致行为不稳定。

这些都偏离 Linux 6.6 的语义（page lock + uptodate + waiters）。


## 设计目标（对齐 Linux 6.6 语义）

- **单一读入者，多等待者**：同一页同时缺页时，只允许一个线程执行 I/O，其它线程等待。
- **就绪与错误状态可见**：页加载完成后，设置 `PG_UPTODATE`；出错设置 `PG_ERROR`，等待者应获取错误。
- **避免“覆盖已有页”**：对同一 pgoff 只允许一次插入，后续并发只共享同一页。
- **不持锁等待**：等待页加载完成时不应持有 `page_cache` 全局锁。
- **写-读冲突有序化**：读缺页与写入/回写在同一页上的并发必须受控，保证读到一致数据或按语义阻塞。


## 建议的状态机与语义

以 Linux page cache 语义为基准，定义“就绪”判定：

- READY = `PG_UPTODATE` 且不含 `PG_LOCKED` 且不含 `PG_ERROR`
- LOADING = `PG_LOCKED`
- ERROR = `PG_ERROR`

状态迁移：

1. 缺页命中空：创建页 -> `PG_LOCKED` -> 读盘 -> 成功：`PG_UPTODATE`，清 `PG_LOCKED`；失败：`PG_ERROR`，清 `PG_LOCKED`。
2. 缺页命中已有页：
   - 若 `PG_UPTODATE`：直接映射
   - 若 `PG_LOCKED`：等待页解锁（等待 `READY/ERROR`）
   - 若 `PG_ERROR`：返回 `SIGBUS`
3. 写-读冲突：
   - 写入页（包括用户写、回写、COW）必须在持有 `PG_LOCKED` 的情况下修改页内容。
   - 读缺页若遇到 `PG_LOCKED`，必须等待写完成，避免读到半写数据。

等待使用 wait queue，解锁时唤醒。


## 具体重构方案（建议步骤）

### 1) 引入“页锁/等待”机制（与 Linux 对齐，但不膨胀 Page 结构体）

在 `Page` 上引入以下能力（复用已有 `PG_LOCKED/PG_WAITERS` 标志），但**不把 wait queue 直接嵌进 `Page`**：

- `lock_page()`：尝试设置 `PG_LOCKED`，若已锁则等待。
- `unlock_page()`：清除 `PG_LOCKED` 并唤醒等待者。
- `wait_on_page_locked()`：等待 `PG_LOCKED` 清除。

实现方式建议（避免结构体膨胀）：

- 新增全局“页等待表”（PageWaitTable），以 **page 的物理地址 paddr（或 page 指针）** 作为 key。
- PageWaitTable 内部用**固定桶哈希**，每个桶持有一个 `WaitQueue + SpinLock`。
- `wait_on_page_locked(page)`：根据 key 选择桶，在桶锁内注册等待；等待条件为 `!PG_LOCKED`。
- `unlock_page(page)`：清 `PG_LOCKED` 后检查 `PG_WAITERS`，若有则在对应桶 `wakeup_all()`。
- `PG_WAITERS` 只作为“是否有等待者”的快速提示位，避免无谓唤醒。

这样把 wait queue 从 `Page` 中移走，避免每个页都持有队列带来的内存膨胀，同时仍保留 Linux 式的 page lock 语义。

#### 关于 key 的选择（paddr vs page 指针 vs page_cache_id+pgoff）

- **paddr**：对页生命周期内是稳定且唯一的（页被释放前不会复用该物理页）。等待者持有 `Arc<Page>` 可保证页不被回收，因此用 paddr 作为 key 是安全的。实现简单，哈希代价低。
- **page 指针**：与 Linux `page_wait_table` 类似，使用 `Arc::as_ptr(&page)` 作为 key，语义上最贴近 Linux。
- **(page_cache_id, pgoff)**：适合“未创建页也要等待”的场景，但当前方案是先创建页再等待，因此不需要该 key。

推荐：**优先用 page 指针**（最贴近 Linux），若实现上使用 paddr 更方便也可接受，但需保证等待者持有 `Arc<Page>`。

#### PageWaitTable 的“膨胀/缩容”策略

采用固定桶数组可以从根本上避免“表无限膨胀”的问题：

- PageWaitTable 在启动时初始化固定大小（例如 256/1024/4096 个桶）。
- 每个桶只包含 `WaitQueue` 和 `SpinLock`，内存占用为常量。
- 等待节点仅在阻塞期间存在，唤醒后自动移除，不会累积。

这与 Linux 的 `page_wait_table` 思路一致：**固定大小 + 哈希冲突可接受**，避免动态扩缩容带来的复杂性。

### 2) PageCache 提供 “lookup + create + lock” 原语

新增统一入口，避免上层重复拼装：

```
PageCache::get_or_create_locked_page(pgoff) -> (Arc<Page>, NeedIo)
```

逻辑建议：

- 持 `page_cache` 锁：
  - 若页存在：
    - 若 `PG_UPTODATE`：返回 `(page, NeedIo = false)`
    - 若 `PG_LOCKED`：标记需要等待（返回待重试）
    - 否则（未锁且未 UPTODATE）：锁页并返回 `NeedIo = true`
  - 若页不存在：创建页并立即置 `PG_LOCKED`，插入后返回 `NeedIo = true`
- 释放 `page_cache` 锁后再等待/读盘，避免持锁睡眠。

### 3) 重构 `filemap_fault()` I/O 路径

目标：只允许一个线程读取，并且把 I/O 直接落到目标页。

建议流程：

1. `get_or_create_locked_page()` 获取页：
   - 若需要等待：调用 `wait_on_page_locked()`，重新检查状态（可循环）
2. 若 `NeedIo = true`：
   - 直接将数据读入该页的内存（替代 `file.pread()` + 临时 buffer）
   - 成功：设置 `PG_UPTODATE`
   - 失败：设置 `PG_ERROR`
   - `unlock_page()` 并唤醒等待者
3. 若 `PG_ERROR`：返回 `VM_FAULT_SIGBUS`
4. 设置 `pfm.page = Some(page)`，进入 `finish_fault()`

**注意**：对齐 Linux 语义，`filemap_fault()` 可在等待时响应 `FAULT_FLAG_INTERRUPTIBLE`，必要时返回 `VM_FAULT_RETRY`。

### 4) `page_cache.read()` / `readahead` 统一使用同一原语

避免“读路径”和“缺页路径”两套逻辑不一致：

- `PageCache::read()` 在准备读缺页时同样使用 `get_or_create_locked_page()`。
- readahead 对创建页应使用 `PG_READAHEAD + PG_LOCKED`，完成后再设置 `PG_UPTODATE`。

### 5) 写路径与读路径的有序化（写-读冲突）

对文件写入路径增加与 pagecache 一致的锁语义：

- `PageCache::write()` 在获取目标页后，应 `lock_page()`，完成写入后 `set PG_UPTODATE` 并 `unlock_page()`。
- 若写入是覆盖写，且页原本 `!PG_UPTODATE`，应先保证页内容完整（可能需要先读盘），再写入用户数据，避免读到未初始化区域。
- `writeback` 对脏页回写时也应持有 `PG_LOCKED` 或等价互斥，避免与读缺页并发修改同一页。

这样可以保证：
- 读缺页遇到写入中的页会等待；
- 写入不会与正在进行的读盘竞争，避免重复 I/O 与不一致数据。

### 6) 完整状态一致性检查点

- `filemap_map_pages()` 只映射 `PG_UPTODATE`，并跳过 `PG_ERROR` 页。
- 删除/回收路径在移除页前确保没有 `PG_LOCKED`，或先等待解锁。


## 风险评估与兼容性

- **与 Linux 语义一致**：采用 “page lock + uptodate + waiters” 模型，行为与 Linux 6.6 接近。
- **性能**：并发缺页合并为单次 I/O，减少重复读；额外等待队列开销可接受。
- **稳定性**：避免重复页插入导致的 `page_manager` / LRU 泄露，提升回收与一致性。


## 结论（你确认后我再动手）

建议按上述步骤引入“页锁 + 等待”机制，统一 `filemap_fault()` 与 `PageCache::read()` 的加载路径，并用 `PG_UPTODATE/PG_ERROR` 做就绪/错误语义。  
确认方案后我再开始具体代码重构。
