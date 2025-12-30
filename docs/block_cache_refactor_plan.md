# DragonOS Block Cache 重构方案

## 当前实现进度

### 已完成 ✅

1. **BufferHead 机制** - `/kernel/src/mm/buffer_head.rs`
   - BhState 状态标志
   - BufferHead 结构和环形链表
   - BufferHeadIter 迭代器
   - create_empty_buffers() 函数

2. **PageCacheOperations trait** - `/kernel/src/mm/page_cache_ops.rs`
   - WritebackControl 回写控制
   - PageCacheOperations trait 定义
   - DefaultPageCacheOps 默认实现
   - FilesystemPageCacheOps 文件系统实现

3. **Page 结构扩展** - `/kernel/src/mm/page.rs`
   - InnerPage 添加 `private` 字段
   - buffers() / attach_buffers() / detach_buffers() 方法

4. **块设备 PageCacheOperations** - `/kernel/src/driver/base/block/blk_page_cache_ops.rs`
   - BlockDevicePageCacheOps 实现
   - block_read_full_folio() / block_write_full_folio() 函数

5. **统一块设备缓存层** - `/kernel/src/driver/base/block/block_dev_cache.rs`
   - BlockDeviceCache 结构
   - 基于 PageCache 的读写实现
   - 全局缓存管理器

6. **集成到 BlockDevice trait** - `/kernel/src/driver/base/block/block_device.rs`
   - 修改 `cache_read` / `cache_write` 使用 BlockDeviceCache
   - 为块设备添加缓存 ID 管理（通过 BlockDevMeta）

7. **移除旧 BlockCache**
   - 已删除 `/kernel/src/driver/block/cache/` 目录
   - 已清理所有相关引用
   - 更新了依赖旧代码的模块（stat.rs, mmc.rs, ahci/mod.rs）

### 待完成 ⏳

1. **PageReclaimer 修改**（可选优化）
   - 识别块设备相关的页面
   - 通过 BufferHead 回写脏页
   - 目前写入采用延迟写模式，脏页在缓存中保留直到手动 sync

## 1. 问题背景

当前 DragonOS 存在 PageCache 和 BlockCache 两套独立的缓存系统，导致：
- 同一份数据可能在两个缓存中都存在
- 回写时产生数据不一致
- 内存使用效率低下

### 当前架构

```
文件读写 → PageCache (HashMap<page_index, Page>)
                ↓ (cache miss 时调用 inode.read_sync)
          ext4/文件系统
                ↓
          another_ext4::BlockDevice
                ↓
          BlockCache (Vec<CacheBlock>, 512B 块)
                ↓
          物理块设备
```

**关键问题**：PageCache 和 BlockCache 完全独立，无同步机制。

## 2. 目标设计

参考 Linux 6.6.21，实现统一的缓存架构：

```
文件读写 → 文件的 AddressSpace/PageCache
                ↓ (通过 BufferHead 映射到块)
块设备读写 → 块设备的 AddressSpace/PageCache
                ↓
          物理块设备
```

**核心思想**：
1. 消除独立的 BlockCache
2. 块设备也拥有自己的 AddressSpace（PageCache）
3. 通过 BufferHead 支持子页面粒度的块操作（512B-4KB）

## 3. 核心数据结构

### 3.1 BufferHead（新增）

位置：`/kernel/src/mm/buffer_head.rs`

```rust
bitflags! {
    pub struct BhState: u32 {
        const BH_UPTODATE = 1 << 0;    // 包含有效数据
        const BH_DIRTY = 1 << 1;        // 已修改
        const BH_LOCK = 1 << 2;         // 被锁定
        const BH_MAPPED = 1 << 3;       // 有磁盘映射
        const BH_NEW = 1 << 4;          // 新创建的映射
        const BH_ASYNC_READ = 1 << 5;   // 异步读进行中
        const BH_ASYNC_WRITE = 1 << 6;  // 异步写进行中
    }
}

/// BufferHead 桥接 Page 和 Block
/// 一个 Page 可以有多个 BufferHead（当 block_size < PAGE_SIZE）
pub struct BufferHead {
    state: AtomicU32,
    this_page: Option<Arc<BufferHead>>,  // 环形链表
    page: Weak<Page>,
    blocknr: u64,                         // 磁盘块号
    size: usize,                          // 块大小
    data_offset: usize,                   // 在页内的偏移
    bdev: Weak<dyn BlockDevice>,
    count: AtomicUsize,
    uptodate_lock: SpinLock<()>,
}
```

### 3.2 扩展 InnerPage

位置：`/kernel/src/mm/page.rs`

```rust
pub struct InnerPage {
    // ... 现有字段 ...

    /// 私有数据 - buffer heads（当 PG_PRIVATE 设置时）
    private: Option<Arc<BufferHead>>,
}
```

### 3.3 AddressSpace（新增）

位置：`/kernel/src/mm/address_space.rs`

```rust
pub struct AddressSpace {
    page_cache: Arc<PageCache>,
    block_size: usize,
    aops: Arc<dyn AddressSpaceOperations>,
    host: AddressSpaceHost,
    private_lock: SpinLock<()>,  // 保护脏位同步
}

pub enum AddressSpaceHost {
    Inode(Weak<dyn IndexNode>),
    BlockDevice(Weak<dyn BlockDevice>),
}

pub trait AddressSpaceOperations: Send + Sync + Debug {
    fn read_folio(&self, page: &Arc<Page>) -> Result<(), SystemError>;
    fn writepages(&self, wbc: &mut WritebackControl) -> Result<(), SystemError>;
    fn dirty_folio(&self, page: &Arc<Page>) -> bool;
    fn release_folio(&self, page: &Arc<Page>) -> bool;
}
```

### 3.4 块设备 AddressSpaceOperations

位置：`/kernel/src/driver/base/block/blk_aops.rs`

```rust
pub struct BlockDeviceAops {
    bdev: Weak<dyn BlockDevice>,
}

impl AddressSpaceOperations for BlockDeviceAops {
    fn read_folio(&self, page: &Arc<Page>) -> Result<(), SystemError> {
        // 创建 buffer heads
        // 计算块范围
        // 调用 bdev.read_at_sync()
        // 标记 buffers 和 page 为 uptodate
    }

    fn writepages(&self, wbc: &mut WritebackControl) -> Result<(), SystemError> {
        // 遍历脏页
        // 通过 buffer heads 写回磁盘
    }

    fn dirty_folio(&self, page: &Arc<Page>) -> bool {
        // 标记所有 buffer heads 为 dirty
        // 标记 page 为 dirty
    }
}
```

## 4. 关键操作流程

### 4.1 块设备读取流程

```
block_read_full_folio(page, get_block_fn)
├─ create_empty_buffers(page, block_size)  // 创建 buffer head 环形链表
├─ for each buffer_head:
│   ├─ get_block(block_nr) → (bdev, disk_block)
│   ├─ bh.map_to_block(bdev, disk_block)
│   ├─ bdev.read_at_sync(disk_block, 1, data)
│   └─ bh.set_uptodate()
└─ page.set_uptodate()
```

### 4.2 块设备写入流程

```
block_write_full_folio(page, wbc)
├─ for each buffer_head:
│   ├─ if bh.is_dirty() && bh.is_mapped():
│   │   ├─ bdev.write_at_sync(bh.blocknr, 1, data)
│   │   └─ bh.clear_dirty()
└─ page.clear_dirty()
```

### 4.3 脏位同步

```rust
fn block_dirty_folio(page: &Arc<Page>) -> bool {
    let _lock = address_space.private_lock.lock();

    // 标记所有 buffers 为 dirty
    for bh in page.buffers() {
        bh.set_dirty();
    }

    // 标记 page 为 dirty
    page.add_flags(PG_DIRTY);
}
```

## 5. 迁移计划

### 阶段 1：基础设施

**任务**：
1. 创建 `/kernel/src/mm/buffer_head.rs`
   - BufferHead 结构
   - BhState 标志
   - 基本操作方法

2. 扩展 `/kernel/src/mm/page.rs`
   - 在 InnerPage 添加 `private: Option<Arc<BufferHead>>`
   - 添加 `buffers()`, `attach_buffers()`, `detach_buffers()` 方法

3. 创建 `/kernel/src/mm/address_space.rs`
   - AddressSpace 结构
   - AddressSpaceOperations trait
   - WritebackControl 结构

### 阶段 2：块设备集成

**任务**：
1. 创建 `/kernel/src/driver/base/block/blk_aops.rs`
   - BlockDeviceAops 实现
   - block_read_full_folio()
   - block_write_full_folio()

2. 修改 `/kernel/src/driver/base/block/block_device.rs`
   - BlockDevice trait 添加 `address_space()` 方法
   - 实现块设备 AddressSpace 初始化

3. 修改块设备读写路径
   - `read_at()` 改为通过 AddressSpace
   - `write_at()` 改为通过 AddressSpace

### 阶段 3：回写机制

**任务**：
1. 修改 `/kernel/src/mm/page.rs` 中的 PageReclaimer
   - 识别块设备页面
   - 通过 buffer heads 回写

2. 添加块设备同步支持
   - `sync()` 方法实现
   - 定期回写机制

### 阶段 4：移除旧代码

**任务**：
1. 移除 `/kernel/src/driver/block/cache/` 目录
2. 清理 BlockDevice trait 中的 `cache_read`, `cache_write`
3. 更新所有使用旧 BlockCache 的代码

## 6. 关键文件清单

| 操作 | 文件路径 |
|------|----------|
| 新增 | `/kernel/src/mm/buffer_head.rs` |
| 新增 | `/kernel/src/mm/address_space.rs` |
| 新增 | `/kernel/src/driver/base/block/blk_aops.rs` |
| 修改 | `/kernel/src/mm/page.rs` |
| 修改 | `/kernel/src/driver/base/block/block_device.rs` |
| 修改 | `/kernel/src/mm/mod.rs` |
| 修改 | `/kernel/src/driver/base/block/mod.rs` |
| 删除 | `/kernel/src/driver/block/cache/` |

## 7. 风险与缓解

### 7.1 性能风险
- **风险**：额外的间接层可能影响性能
- **缓解**：关键路径使用 inline，buffer head 使用对象池

### 7.2 内存开销
- **风险**：BufferHead 结构增加内存占用
- **缓解**：4KB 块时不创建 buffer head；内存紧张时释放 clean pages 的 buffers

### 7.3 死锁风险
- **风险**：多锁（page_lock, private_lock, buffer_lock）可能死锁
- **缓解**：严格定义锁顺序：page_lock → private_lock → buffer_lock

### 7.4 兼容性风险
- **风险**：现有文件系统可能受影响
- **缓解**：IndexNode trait 保持不变，AddressSpace 是内部抽象

## 8. 测试策略

1. **单元测试**
   - BufferHead 创建和链接
   - Page 与 BufferHead 关联
   - 脏位同步

2. **集成测试**
   - 块设备读写通过新缓存
   - ext4 挂载和文件操作
   - 回写机制验证

3. **压力测试**
   - 高并发 I/O
   - 内存压力下的行为
   - 混合文件和块设备 I/O
