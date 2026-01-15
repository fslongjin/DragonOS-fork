# DragonOS Block Cache 重构方案

## 问题分析

当前DragonOS的block cache实现存在以下严重问题：

### 1. 元数据开销过大（核心问题）

当前设计以512字节为缓存单位，每个CacheBlock需要：
- `Box<[u8]>`: 512B数据 + 堆分配头（16-32字节）
- `CacheBlockFlag`: 状态标志
- `BlockId`: LBA地址
- HashMap节点: 约48字节（key + value + 链表指针）

**元数据开销约10-20%**，对于512字节的小块来说非常浪费。

对比Linux buffer_head：约120字节管理512字节数据，开销约25%。Linux官方注释明确说这是**遗留设计**。

### 2. Slab分配器碎片问题

每个512字节块都需要：
- 单独的`Box<[u8]>`分配
- 单独的HashMap节点分配

大量小对象分配导致slab内部碎片严重。

### 3. 与Page Cache功能重复

当前数据流：
```
Ext4Inode::read_at()
  -> PageCache::read() [4KB缓存，已有LRU]
    -> inode.read_sync()
      -> another_ext4::read()
        -> GenDisk::read_at_bytes()
          -> BlockDevice::read_at()
            -> cache_read() [512B缓存，重复！]
```

Page Cache已经是4KB粒度的缓存，Block Cache又是512B粒度，两层缓存功能重复。

### 4. 其他问题

- **全局共享缓存**：不同设备的相同LBA会冲突
- **cache_read暴露给上层**：破坏封装性
- **简陋的淘汰机制**：Round-Robin比FIFO还简陋

## 重构方案选择

### 方案A：完全删除Block Cache

**理由**：
1. Page Cache已经在文件级别做了4KB粒度的缓存
2. 文件系统元数据（superblock, inode table）由another_ext4库内部管理
3. Block Cache与Page Cache功能重复，删除后简化架构

**实现**：
- `BlockDevice::read_at()` 直接调用 `read_at_sync()`
- 删除整个 `kernel/src/driver/block/cache/` 目录
- 删除 `cache_read/cache_write` 方法

### 方案B：改为Page粒度缓存（推荐 ✓）

如果确实需要块设备级别的缓存（如直接块设备访问），改为4KB粒度。

**核心设计**：以4KB Page为缓存单位，而不是512B Block。

**优势**：
- 元数据开销从15%降到0.8%
- 使用固定大小数组避免slab碎片
- 预分配物理页帧，无动态内存分配
- LRU使用数组索引，无指针开销

## 新架构设计（方案B详细设计）

```
┌─────────────────────────────────────────────────────────┐
│              文件系统层 (ext4, FAT等)                    │
│         使用 GenDisk::read_at_bytes/write_at_bytes      │
└─────────────────────────────────────────────────────────┘
                           │
                           ▼
┌─────────────────────────────────────────────────────────┐
│                    GenDisk 层                            │
│              分区抽象、LBA偏移转换                        │
│           调用 BlockDevice::read_at/write_at             │
└─────────────────────────────────────────────────────────┘
                           │
                           ▼
╔═════════════════════════════════════════════════════════╗
║           新: Per-Device Block Cache 层                  ║
║  - 每个BlockDevice独立的缓存实例                         ║
║  - 对上层完全透明                                        ║
║  - LRU淘汰策略                                          ║
║  - Cache miss时自动提交BIO                              ║
╚═════════════════════════════════════════════════════════╝
                           │
                           ▼
┌─────────────────────────────────────────────────────────┐
│                      BIO 层                              │
│    BioRequest: 异步I/O请求 (已有实现, bio.rs)            │
│    Completion等待机制                                    │
└─────────────────────────────────────────────────────────┘
                           │
                           ▼
┌─────────────────────────────────────────────────────────┐
│                  块设备驱动                              │
│      VirtIOBlkDevice, AhciDisk (submit_bio实现)         │
└─────────────────────────────────────────────────────────┘
```

## 核心数据结构（高效设计）

### 设计原则

1. **以4KB Page为缓存单位**，而不是512B Block
2. **使用物理页帧**作为数据存储，不需要额外的Box<[u8]>
3. **固定大小数组**存储缓存槽位，避免动态内存分配
4. **紧凑的元数据结构**，最小化开销

### 元数据开销对比

| 设计 | 缓存单位 | 元数据/单位 | 开销比例 |
|------|---------|------------|---------|
| 当前设计 | 512B | ~80字节 | ~15% |
| 新设计 | 4KB | ~32字节 | ~0.8% |

### DevicePageCache (每设备缓存)

```rust
// kernel/src/driver/base/block/page_cache/mod.rs

/// 缓存槽位数量（512个槽位 = 2MB缓存）
const CACHE_SLOTS: usize = 512;
const PAGE_SIZE: usize = 4096;

pub struct DevicePageCache {
    /// 弱引用设备
    device: Weak<dyn BlockDevice>,
    /// 缓存槽位数组（固定大小，一次性分配）
    slots: Box<[CacheSlot; CACHE_SLOTS]>,
    /// 页索引 -> 槽位索引 映射
    index_map: RwLock<HashMap<u64, u16>>,
    /// LRU链表头尾（使用数组索引）
    lru_head: AtomicU16,
    lru_tail: AtomicU16,
    /// 已使用槽位数
    used_slots: AtomicU16,
    /// 统计信息
    stats: CacheStats,
}

/// 单个缓存槽位 - 紧凑设计
#[repr(C)]
struct CacheSlot {
    /// 页索引 (字节偏移 / 4096)，u64::MAX表示空槽
    page_index: AtomicU64,        // 8字节
    /// 物理页帧地址
    phys_frame: AtomicUsize,      // 8字节
    /// 状态标志
    flags: AtomicU8,              // 1字节
    /// LRU前驱槽位索引
    lru_prev: AtomicU16,          // 2字节
    /// LRU后继槽位索引
    lru_next: AtomicU16,          // 2字节
    /// 引用计数（用于并发访问）
    ref_count: AtomicU8,          // 1字节
    _padding: [u8; 10],           // 对齐到32字节
}
// 总共32字节，管理4KB数据，开销约0.8%

/// 槽位状态
const SLOT_EMPTY: u8 = 0;
const SLOT_VALID: u8 = 1;
const SLOT_DIRTY: u8 = 2;
const SLOT_LOCKED: u8 = 4;
```

### 内存布局

```
DevicePageCache (一次性分配)
├── slots: Box<[CacheSlot; 512]>  // 512 * 32 = 16KB 元数据
├── index_map: HashMap            // 动态，约8KB
└── 物理页帧池                     // 512 * 4KB = 2MB 数据
                                  // 总开销: 24KB / 2MB ≈ 1.2%
```

### 物理页帧管理

```rust
impl DevicePageCache {
    /// 初始化时预分配所有物理页帧
    pub fn new(device: Weak<dyn BlockDevice>) -> Result<Arc<Self>, SystemError> {
        let mut slots = Box::new([CacheSlot::empty(); CACHE_SLOTS]);

        // 预分配物理页帧
        for slot in slots.iter_mut() {
            let frame = LockedFrameAllocator.allocate_one()?;
            slot.phys_frame.store(frame.data(), Ordering::Relaxed);
        }

        Ok(Arc::new(Self {
            device,
            slots,
            index_map: RwLock::new(HashMap::new()),
            lru_head: AtomicU16::new(u16::MAX),
            lru_tail: AtomicU16::new(u16::MAX),
            used_slots: AtomicU16::new(0),
            stats: CacheStats::new(),
        }))
    }
}
```

## 关键接口变更

### BlockDevice Trait 修改

```rust
// block_device.rs

pub trait BlockDevice: Device {
    // 保留: 底层同步IO
    fn read_at_sync(...) -> Result<usize, SystemError>;
    fn write_at_sync(...) -> Result<usize, SystemError>;
    fn submit_bio(...) -> Result<(), SystemError>;

    // 新增: 获取设备页缓存（懒初始化）
    fn page_cache(&self) -> Option<Arc<DevicePageCache>> {
        None  // 默认无缓存，由具体驱动实现
    }

    // 修改: read_at/write_at 自动使用缓存
    fn read_at(...) -> Result<usize, SystemError> {
        if let Some(cache) = self.page_cache() {
            cache.read(lba_id_start, count, buf)
        } else {
            self.read_at_sync(lba_id_start, count, buf)
        }
    }

    // 删除: cache_read, cache_write (不再暴露)
}
```

## Cache Read/Write 流程

### Read流程 (Page粒度)

```
DevicePageCache::read(byte_offset, len, buf)
    │
    ├─ 1. 计算涉及的页索引范围
    │     page_start = byte_offset / 4096
    │     page_end = (byte_offset + len - 1) / 4096
    │
    ├─ 2. 对每个页检查缓存
    │     for page_idx in page_start..=page_end:
    │         if let Some(slot) = index_map.get(page_idx):
    │             从slot.phys_frame复制数据到buf
    │             touch_lru(slot)  // 移到LRU头部
    │         else:
    │             记录miss的页
    │
    ├─ 3. 批量读取miss的页
    │     合并连续的miss页为单个BIO请求
    │     submit_bio_read() -> wait()
    │
    ├─ 4. 将读取的数据填入缓存槽位
    │     for each miss_page:
    │         slot = alloc_slot()  // 可能触发LRU淘汰
    │         复制数据到slot.phys_frame
    │         index_map.insert(page_idx, slot_idx)
    │
    └─ 5. 返回数据
```

### Write流程 (Write-Through)

```
DevicePageCache::write(byte_offset, len, buf)
    │
    ├─ 1. 提交BIO写入设备
    │     submit_bio_write() -> wait()
    │
    └─ 2. 更新缓存（如果页在缓存中）
          for each affected page:
              if let Some(slot) = index_map.get(page_idx):
                  更新slot.phys_frame中的数据
                  touch_lru(slot)
```

## LRU淘汰机制（数组索引实现）

```rust
impl DevicePageCache {
    /// 分配一个缓存槽位，必要时淘汰LRU尾部
    fn alloc_slot(&self) -> u16 {
        // 优先使用空闲槽位
        if self.used_slots.load(Relaxed) < CACHE_SLOTS as u16 {
            let slot_idx = self.used_slots.fetch_add(1, Relaxed);
            self.lru_push_front(slot_idx);
            return slot_idx;
        }

        // 淘汰LRU尾部
        let evict_idx = self.lru_tail.load(Relaxed);
        let slot = &self.slots[evict_idx as usize];

        // 从index_map中移除旧映射
        let old_page_idx = slot.page_index.load(Relaxed);
        self.index_map.write().remove(&old_page_idx);

        // 移到LRU头部
        self.lru_move_to_front(evict_idx);
        self.stats.evictions.fetch_add(1, Relaxed);

        evict_idx
    }

    /// 将槽位移到LRU头部（使用数组索引，无指针）
    fn lru_move_to_front(&self, slot_idx: u16) {
        let slot = &self.slots[slot_idx as usize];
        let prev = slot.lru_prev.load(Relaxed);
        let next = slot.lru_next.load(Relaxed);

        // 从当前位置移除
        if prev != u16::MAX {
            self.slots[prev as usize].lru_next.store(next, Relaxed);
        }
        if next != u16::MAX {
            self.slots[next as usize].lru_prev.store(prev, Relaxed);
        }
        if self.lru_tail.load(Relaxed) == slot_idx {
            self.lru_tail.store(prev, Relaxed);
        }

        // 插入头部
        let old_head = self.lru_head.swap(slot_idx, Relaxed);
        slot.lru_prev.store(u16::MAX, Relaxed);
        slot.lru_next.store(old_head, Relaxed);
        if old_head != u16::MAX {
            self.slots[old_head as usize].lru_prev.store(slot_idx, Relaxed);
        }
    }
}
```

**优点**：
- 使用数组索引（u16）而不是指针，节省内存
- 无需额外的LruNode结构
- O(1)的LRU操作

## 文件变更清单

### 新增文件

| 文件 | 功能 |
|------|------|
| `kernel/src/driver/base/block/page_cache/mod.rs` | DevicePageCache主结构 |
| `kernel/src/driver/base/block/page_cache/slot.rs` | CacheSlot结构 |

### 修改文件

| 文件 | 变更 |
|------|------|
| `kernel/src/driver/base/block/mod.rs` | 添加 `pub mod page_cache;` |
| `kernel/src/driver/base/block/block_device.rs` | 删除cache_read/cache_write，添加page_cache()方法 |
| `kernel/src/driver/block/virtio_blk.rs` | 实现page_cache()方法 |
| `kernel/src/driver/disk/ahci/ahcidisk.rs` | 实现page_cache()方法 |
| `kernel/src/driver/disk/ahci/mod.rs` | 删除BlockCache::init()调用 |

### 删除文件

| 文件 | 原因 |
|------|------|
| `kernel/src/driver/block/cache/mod.rs` | 旧缓存模块 |
| `kernel/src/driver/block/cache/cached_block_device.rs` | 旧BlockCache实现 |
| `kernel/src/driver/block/cache/cache_block.rs` | 旧CacheBlock |
| `kernel/src/driver/block/cache/cache_iter.rs` | 旧迭代器 |

## 迁移策略 (平滑过渡)

### Phase 1: 创建新缓存模块（不破坏现有代码）
1. 创建 `kernel/src/driver/base/block/page_cache/` 目录
2. 实现 DevicePageCache, CacheSlot
3. 在BlockDevice trait添加 `fn page_cache(&self) -> Option<...>` 默认返回None

### Phase 2: 迁移驱动
1. VirtIOBlkDevice 实现 page_cache() 返回新缓存
2. AhciDisk 实现 page_cache()
3. 测试验证新缓存工作正常

### Phase 3: 切换默认实现
1. 修改 read_at/write_at 默认实现使用新缓存
2. 标记 cache_read/cache_write 为 deprecated

### Phase 4: 清理
1. 删除 cache_read/cache_write 方法
2. 删除旧 `kernel/src/driver/block/cache/` 模块
3. 删除 AHCI 中的 BlockCache::init() 调用

## 验证方案

1. **单元测试**
   - LRU链表操作正确性
   - 缓存命中/未命中逻辑
   - 淘汰机制测试

2. **集成测试**
   - 挂载ext4文件系统，读写文件
   - 验证缓存统计（命中率、淘汰次数）
   - 并发读写测试

3. **性能测试**
   - 对比重构前后的IO性能
   - 验证缓存命中率提升

## 关键实现细节

### 缓存与BlockDevMeta绑定

```rust
// manager.rs
pub struct BlockDevMeta {
    // ... 现有字段 ...
    page_cache: OnceCell<Arc<DevicePageCache>>,
}

impl BlockDevMeta {
    pub fn get_or_init_cache(&self, device: Weak<dyn BlockDevice>)
        -> Result<Arc<DevicePageCache>, SystemError>
    {
        self.page_cache.get_or_try_init(|| {
            DevicePageCache::new(device)
        }).cloned()
    }
}
```

### 与现有BIO框架集成

新缓存直接使用现有的 `submit_bio_read/submit_bio_write`（`block_device.rs:438-475`）：
- 优先使用异步BIO
- 不支持异步时自动回退到同步

无需修改BIO框架本身。
