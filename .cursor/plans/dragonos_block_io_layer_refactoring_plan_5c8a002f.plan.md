---
name: DragonOS Block IO Layer Refactoring Plan
overview: Refactor the Block IO layer to support asynchronous IO, request merging, and DMA, replacing the current synchronous block cache implementation while maintaining compatibility with the existing VFS and PageCache.
todos:
  - id: bio-struct
    content: 创建 Bio 和 BioVec 结构体 (kernel/src/driver/block/bio.rs)
    status: pending
  - id: req-struct
    content: 创建 Request 结构体和合并逻辑 (kernel/src/driver/block/request.rs)
    status: pending
    dependencies:
      - bio-struct
  - id: async-trait
    content: 定义 AsyncBlockDevice 接口并更新 BlockDevice trait
    status: pending
    dependencies:
      - bio-struct
  - id: scheduler
    content: 实现 NoopScheduler 和 RequestQueue (kernel/src/driver/block/scheduler/)
    status: pending
    dependencies:
      - req-struct
  - id: virtio-refactor
    content: 重构 VirtIO-Blk 驱动以支持 RequestQueue 和 DMA
    status: pending
    dependencies:
      - scheduler
      - async-trait
  - id: integration
    content: 在 VirtIOBlkDevice 中集成 submit_bio 并适配同步接口
    status: pending
    dependencies:
      - virtio-refactor
---

# DragonOS Block IO Layer Refactoring Plan

## 1. 目标

重构 DragonOS 的 Block IO 层，解决当前 Block Cache 机制不支持异步 IO、缺乏请求合并、无法使用 DMA 等问题。**注意**：本计划**不涉及**重写现有的 `PageCache`，而是替换底层的 Block IO 引擎，使其能被现有的 `PageCache` 和文件系统高效利用。

## 2. 核心架构设计

### 2.1 新引入的组件

1.  **Bio (Block IO)**: 描述基本的 IO 请求，支持 Scatter-Gather (向量化 IO)。
2.  **Request**: 调度器层的请求单位，包含一个或多个合并后的 `Bio`。
3.  **RequestQueue**: 负责 IO 调度、请求合并和限流。
4.  **AsyncBlockDevice Trait**: 提供异步提交 Bio 的接口。

### 2.2 数据流变迁

**当前**:`VFS/PageCache` -> `BlockDevice::read_at_sync` -> `VirtIOBlk::read_blocks` (Sync, No Merge, Copy overhead)**重构后**:`VFS/PageCache` -> `BlockDevice::read_at_sync` (Legacy Wrapper) -> `make_request` -> `RequestQueue` (Merge) -> `VirtIOBlk` (DMA + Async)*注：虽然上层暂时保持同步调用，但底层将获得请求合并和 DMA 的性能优势。未来 VFS 可直接调用异步接口。*---

## 3. 详细实施步骤

### 阶段一：定义核心数据结构 (Core Structures)

位置：`kernel/src/driver/block/`

1.  **创建 `bio.rs`**:

    -   `BioVec`: `(Arc<Page>, offset: usize, len: usize)`，描述内存片段。
    -   `Bio`: 包含 `sector`, `size`, `operation` (Read/Write), `Vec<BioVec>`, `priority`, `callback/waker`。
    -   支持从 `buf: &[u8] `构建 Bio (涉及内存拷贝到 Page) 和从 `Vec<Arc<Page>>` 构建 (Zero Copy)。

2.  **创建 `request.rs`**:

    -   `Request`: 包含 `Vec<Bio>`, `total_len`, `sector`。
    -   实现 `can_merge(bio)` 方法判断是否可合并。

### 2. 阶段二：实现 IO 调度层 (IO Scheduler)

位置：`kernel/src/driver/block/scheduler/`

1.  **定义 `Scheduler` Trait**:

    -   `submit_bio(bio)`: 提交请求。
    -   `get_request()`: 驱动获取下一个请求。

2.  **实现 `NoopScheduler`**:

    -   简单的 FIFO 队列，支持基本的 LBA 连续合并。
    -   位置：`kernel/src/driver/block/scheduler/noop.rs`

3.  **RequestQueue**:

    -   持有 `Scheduler` 实例。
    -   提供 `make_request(bio)` 接口。

### 3. 阶段三：定义异步块设备接口

位置：`kernel/src/driver/base/block/`

1.  **更新 `BlockDevice` Trait** 或新增 `AsyncBlockDevice` Trait:

    -   增加 `submit_bio(bio) -> Future/Result`。
    -   现有的 `read_at_sync` 默认实现改为：构建 Bio -> submit_bio -> wait for completion。

### 4. 阶段四：重构 VirtIO-Blk 驱动

位置：`kernel/src/driver/block/virtio_blk.rs`

1.  **引入 `virtio-drivers` 的异步机制**:

    -   使用 `virtio-drivers` crate 的异步接口 (如果有) 或手动管理 virtqueue 描述符链。
    -   支持 Scatter-Gather List (SGL)，直接将 `BioVec` 中的物理页地址填入描述符，**实现 Zero-Copy DMA**。

2.  **实现中断处理**:

    -   在中断处理函数中处理 Request 完成事件，唤醒对应的 Bio Waker。

3.  **对接 RequestQueue**:

    -   驱动初始化时创建 RequestQueue。
    -   后台任务或中断驱动循环从 Queue 获取 Request 并提交给硬件。

### 5. 阶段五：集成与清理

1.  **适配 `VirtIOBlkDevice`**:

    -   实现新的 `submit_bio` 接口。
    -   保留 `read_at_sync` 但将其重定向到新路径。

2.  **移除旧 BlockCache**:

    -   确认所有路径都走新层后，移除 `kernel/src/driver/block/cache/` 下的代码。

---

## 4. 文件变更清单

-   New: `kernel/src/driver/block/bio.rs`
-   New: `kernel/src/driver/block/request.rs`
-   New: `kernel/src/driver/block/scheduler/mod.rs`
-   New: `kernel/src/driver/block/scheduler/noop.rs`
-   Modify: `kernel/src/driver/base/block/block_device.rs` (Add submit_bio)
-   Modify: `kernel/src/driver/block/virtio_blk.rs` (Rewrite for Async/DMA)