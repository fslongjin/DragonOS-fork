# DragonOS Block Cache 机制设计文档 （主线上游实现）

## 目录

1. [概述](#1-概述)
2. [架构设计](#2-架构设计)
3. [核心数据结构](#3-核心数据结构)
4. [缓存策略](#4-缓存策略)
5. [读操作流程](#5-读操作流程)
6. [写操作流程](#6-写操作流程)
7. [并发控制](#7-并发控制)
8. [文件结构](#8-文件结构)
9. [优化建议](#9-优化建议)

---

## 1. 概述

### 1.1 设计目标

DragonOS的Block Cache机制位于块设备驱动之上，为文件系统和其他上层组件提供透明的块级缓存服务，主要目标是：

- **减少磁盘I/O**：通过缓存频繁访问的块，减少对底层物理设备的访问次数
- **提高访问速度**：内存访问速度远快于磁盘访问
- **透明性**：上层调用者无需关心缓存实现细节

### 1.2 核心参数

| 参数 | 值 | 说明 |
|------|-----|------|
| `BLOCK_SIZE` | 512 字节 | 块大小，与磁盘扇区对齐 |
| `BLOCK_SIZE_LOG` | 9 | 块大小的对数值（log₂(512)=9） |
| `CACHE_THRESHOLD` | 2 MB | 缓存总容量上限 |

缓存最大块数计算：
```
max_blocks = CACHE_THRESHOLD * (1 << (20 - BLOCK_SIZE_LOG))
           = 2 * (1 << (20 - 9))
           = 2 * 2048
           = 4096 块
```

---

## 2. 架构设计

### 2.1 整体架构

```
┌─────────────────────────────────────────────────────────────┐
│                        上层调用者                             │
│                   (文件系统/应用程序)                         │
└────────────────────────────┬────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                   BlockDevice Trait                          │
│   - cache_read()  - cache_write()                           │
│   - read_at()    - write_at()                              │
│   - read_at_bytes() - write_at_bytes()                     │
└────────────────────────────┬────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                      BlockCache                              │
│  ┌─────────────────────────────────────────────────────┐   │
│  │           核心组件                                   │   │
│  │  ┌──────────────┐      ┌──────────────────────┐     │   │
│  │  │ CacheSpace   │◄────┤  CacheMapper          │     │   │
│  │  │              │      │  (LBA → CacheAddr)    │     │   │
│  │  │ 存储缓存块   │      └──────────────────────┘     │   │
│  │  └──────────────┘                                  │   │
│  │           ▲                                         │   │
│  │           │                                         │   │
│  │  ┌────────┴────────┐                               │   │
│  │  │ FrameSelector   │                               │   │
│  │  │ (替换算法)       │                               │   │
│  │  └─────────────────┘                               │   │
│  └─────────────────────────────────────────────────────┘   │
└────────────────────────────┬────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                    块设备驱动层                              │
│  (AHCI/VirtIO-Block/RAM Disk等)                             │
└─────────────────────────────────────────────────────────────┘
```

### 2.2 核心组件关系

```
                    BlockCache (对外接口)
                           │
           ┌───────────────┼───────────────┐
           ▼               ▼               ▼
    CacheSpace      CacheMapper      BlockIter
    (存储管理)       (映射管理)       (迭代器)
           │
           ▼
    FrameSelector
    (替换算法)
```

---

## 3. 核心数据结构

### 3.1 CacheBlock - 缓存块单元

**位置**: `kernel/src/driver/block/cache/cache_block.rs`

```rust
pub struct CacheBlock {
    data: Box<[u8]>,           // 512字节的块数据
    _flag: CacheBlockFlag,     // 标志位（当前未使用）
    lba_id: BlockId,           // 对应的逻辑块地址
}
```

**主要方法**:
- `from_slice()`: 从切片创建缓存块
- `write_data()`: 覆盖写数据（用于write-through更新）
- `data()`: 读取数据到指定buffer

### 3.2 CacheSpace - 缓存空间管理

**位置**: `kernel/src/driver/block/cache/cached_block_device.rs:245`

```rust
struct CacheSpace {
    root: Vec<CacheBlock>,              // 缓存块的实际存储
    frame_selector: Box<dyn FrameSelector>, // 替换算法实例
}
```

**职责**:
- 存储所有缓存块数据
- 执行缓存的读写操作
- 执行块的插入和替换

### 3.3 CacheMapper - 地址映射

**位置**: `kernel/src/driver/block/cache/cached_block_device.rs:367`

```rust
struct CacheMapper {
    map: HashMap<BlockId, CacheBlockAddr>,  // LBA → Cache地址
}
```

**职责**:
- 建立LBA ID到Cache内部地址的映射
- 快速查找块是否在缓存中
- 管理映射的插入和删除

### 3.4 FrameSelector - 替换算法接口

**位置**: `kernel/src/driver/block/cache/cached_block_device.rs:400`

```rust
trait FrameSelector {
    /// 获取append操作的index（缓存未满时）
    fn index_append(&mut self) -> CacheBlockAddr;

    /// 获取replace操作的index（缓存满时）
    fn index_replace(&mut self) -> CacheBlockAddr;

    /// 判断是否可以append
    fn can_append(&self) -> bool;

    /// 获取当前size
    fn size(&self) -> usize;
}
```

### 3.5 SimpleFrameSelector - 简单循环替换算法

**位置**: `kernel/src/driver/block/cache/cached_block_device.rs:417`

```rust
struct SimpleFrameSelector {
    threshold: usize,    // 缓存容量上限（块数）
    size: usize,         // 当前已用块数
    current: usize,      // 当前替换位置
}
```

**替换策略**: 循环替换 `0, 1, 2, ..., threshold-1, 0, 1, ...`

---

## 4. 缓存策略

### 4.1 替换算法

当前实现：**SimpleFrameSelector**（类循环算法）

| 状态 | 操作 |
|------|------|
| `size < threshold` | append: 直接添加到末尾 |
| `size >= threshold` | replace: 循环替换 |

**特点**:
- 简单高效，O(1)时间复杂度
- 无需维护额外元数据
- 缺点：不考虑访问热度，可能替换热点数据

### 4.2 写策略：Write-Through

```rust
pub fn immediate_write(
    lba_id_start: BlockId,
    count: usize,
    data: &[u8],
) -> Result<usize, BlockCacheError>
```

**流程**:
1. 检查块是否在缓存中
2. 如果命中：更新缓存内容
3. 如果未命中：插入新块到缓存
4. **立即**写入底层设备

**特点**:
- 数据一致性最好
- 写性能相对较低
- 不需要dirty位跟踪

### 4.3 读策略：Cache-Aside

```rust
pub fn read(
    lba_id_start: BlockId,
    count: usize,
    buf: &mut [u8],
) -> Result<usize, BlockCacheError>
```

**流程**:
1. 先检查缓存是否命中
2. 命中：直接从缓存返回
3. 未命中：从设备读取，填充缓存，再返回

---

## 5. 读操作流程

### 5.1 完整流程图

```
┌─────────────────────────────────────────────────────────────────┐
│                    cache_read(lba, count, buf)                  │
└────────────────────────────────────┬────────────────────────────┘
                                     │
                                     ▼
┌─────────────────────────────────────────────────────────────────┐
│                    BlockCache::read()                           │
│  1. 创建 BlockIter                                              │
│  2. 调用 check_able_to_read() 检查缓存                          │
└────────────────────────────┬────────────────────────────────────┘
                             │
                 ┌───────────┴───────────┐
                 │                       │
                 ▼                       ▼
        ┌─────────────────┐     ┌─────────────────────────┐
        │   全部命中       │     │   部分缺失/全部缺失      │
        │                 │     │                         │
        │  直接从缓存读取  │     │  返回 BlockFaultError   │
        └─────────────────┘     └───────────┬─────────────┘
                                           │
                                           ▼
                                ┌─────────────────────────┐
                                │  调用 read_at_sync()    │
                                │  从底层设备读取          │
                                └───────────┬─────────────┘
                                            │
                                            ▼
                                ┌─────────────────────────┐
                                │  BlockCache::insert()   │
                                │  将缺失块插入缓存        │
                                └─────────────────────────┘
```

### 5.2 关键代码路径

**路径1: 缓存命中**

```
BlockCache::read()
  └─> check_able_to_read()
       └─> mapper.find() ──> 全部找到
  └─> read_one_block() × N
       └─> space.read()
            └─> root[addr].data()
```

**路径2: 缓存未命中**

```
BlockCache::read()
  └─> check_able_to_read()
       └─> mapper.find() ──> 部分缺失
  └─> 返回 BlockFaultError(FailData[])
       │
       ▼ (在 BlockDevice::cache_read 中处理)
  read_at_sync() ──> 从设备读取
       │
       ▼
  BlockCache::insert(FailData[], data)
  └─> insert_one_block() × N
       └─> space.insert()
            ├─> can_append()? ─YES─> frame_selector.index_append()
            │                              └─> root.push()
            │                              └─> mapper.insert()
            │
            └─NO─> frame_selector.index_replace()
                         └─> 替换 root[index]
                         └─> mapper.insert(new_lba)
                         └─> mapper.remove(old_lba)
```

### 5.3 BlockIter 块迭代器

用于处理连续块的读取操作：

```rust
pub struct BlockIter {
    lba_id_start: BlockId,    // 起始LBA
    count: usize,             // 块数量
    current: usize,           // 当前遍历位置
    block_size: usize,        // 块大小
}
```

**作用**:
- 将请求分解为单独的块
- 为每个块生成 `BlockData` (包含lba_id和位置信息)
- 支持缺块信息的收集 (`FailData`)

---

## 6. 写操作流程

### 6.1 写流程图

```
┌─────────────────────────────────────────────────────────────────┐
│                cache_write(lba, count, buf)                     │
└────────────────────────────────────┬────────────────────────────┘
                                     │
                                     ▼
┌─────────────────────────────────────────────────────────────────┐
│              BlockCache::immediate_write()                      │
│  1. 遍历每个块                                                  │
│  2. 检查 mapper.find(lba_id)                                    │
└────────────────────────────┬────────────────────────────────────┘
                             │
                 ┌───────────┴───────────┐
                 │                       │
                 ▼                       ▼
        ┌─────────────────┐     ┌─────────────────────────┐
        │    命中缓存      │     │   未命中缓存             │
        │                 │     │                         │
        │  space.write()  │     │  space.insert()         │
        │  更新缓存内容    │     │  插入新块               │
        └─────────────────┘     └─────────────────────────┘
                 │                       │
                 └───────────┬───────────┘
                             │
                             ▼
                ┌─────────────────────────┐
                │  write_at_sync()        │
                │  写入底层设备            │
                └─────────────────────────┘
```

### 6.2 Write-Through 特点

```
┌──────────────────────────────────────────────────────┐
│                  写入流程                             │
│                                                      │
│   上层写入                                          │
│      │                                              │
│      ▼                                              │
│   更新Cache  ─────────────────┐                     │
│      │                        │                     │
│      ▼                        │                     │
│   写入设备  ◄─────────────────┘                     │
│      │                                              │
│      ▼                                              │
│   返回                                             │
│                                                      │
│   特点：Cache和设备同时保持最新                      │
└──────────────────────────────────────────────────────┘
```

---

## 7. 并发控制

### 7.1 锁机制

```rust
struct LockedCacheSpace(RwLock<CacheSpace>);

struct LockedCacheMapper {
    lock: RwLock<CacheMapper>,
}
```

**锁策略**:
- **读操作**: 使用读锁，允许多个并发读
- **写操作**: 使用写锁，独占访问

### 7.2 并发安全性

| 操作 | 锁类型 | 并发性 |
|------|--------|--------|
| `read()` | Read Lock | 多个读者可并发 |
| `insert()` | Write Lock | 独占 |
| `immediate_write()` | Write Lock | 独占 |
| `find()` (在mapper中) | Read Lock | 多个读者可并发 |

---

## 8. 文件结构

```
kernel/src/driver/block/cache/
├── mod.rs                       # 模块入口，常量和错误定义
├── cache_block.rs              # CacheBlock 结构体
├── cache_iter.rs               # 块迭代器和缺块信息
└── cached_block_device.rs      # 核心缓存实现

kernel/src/driver/base/block/
└── block_device.rs             # BlockDevice trait定义
```

### 8.1 模块依赖关系

```
block_device.rs
    │
    ├─> cache/mod.rs
    │       │
    │       ├─> cache/cache_block.rs
    │       ├─> cache/cache_iter.rs
    │       └─> cache/cached_block_device.rs
    │
    └─> disk_info.rs, gendisk.rs, manager.rs
```

---

## 9. 优化建议

### 9.1 当前实现的局限

| 方面 | 当前实现 | 改进方向 |
|------|----------|----------|
| 替换算法 | 简单循环 | 实现LRU/ARC/Clock算法 |
| 写策略 | Write-Through | 支持Write-Back（需要dirty标记） |
| 容量 | 固定2MB | 支持动态调整 |
| 预读取 | 基础支持 | 智能预取（顺序检测） |
| 统计 | 无 | 命中率统计、性能监控 |

### 9.2 代码中预留的扩展点

**CacheBlockFlag** (cache_block.rs:10):
```rust
pub enum CacheBlockFlag {
    Unused,
    Unwrited,    // 预留用于写回策略
    Writed,
}
```
当前未使用，为将来实现Write-Back预留。

**FrameSelector Trait** (cached_block_device.rs:400):
```rust
trait FrameSelector { ... }
```
替换算法可插拔，只需实现此trait即可切换算法。

### 9.3 实现LRU替换算法示例框架

```rust
struct LRUFrameSelector {
    threshold: usize,
    size: usize,
    // 使用双向链表+HashMap实现
    access_list: LinkedList<BlockId>,
    access_map: HashMap<BlockId, Node<BlockId>>,
}

impl FrameSelector for LRUFrameSelector {
    fn index_replace(&mut self) -> CacheBlockAddr {
        // 返回最久未使用块的地址
        let lru_lba = self.access_list.pop_front()?;
        // ... 返回对应的cache地址
    }

    // 记录访问需要在每次read时调用
    fn record_access(&mut self, lba_id: BlockId) {
        // 更新访问顺序
    }
}
```

---

## 附录：常量定义

```rust
// kernel/src/driver/block/cache/mod.rs

/// 块大小的对数值
pub const BLOCK_SIZE_LOG: usize = 9;

/// 块大小（512字节）
pub const BLOCK_SIZE: usize = 1 << BLOCK_SIZE_LOG;

/// 缓存阈值（2MB）
pub const CACHE_THRESHOLD: usize = 2;

// kernel/src/driver/base/block/block_device.rs

/// LBA大小（字节）
pub const LBA_SIZE: usize = 512;

/// 块大小上限（2^12 = 4096字节）
pub const BLK_SIZE_LOG2_LIMIT: u8 = 12;
```

---

**文档版本**: v1.0
**生成日期**: 2025-12-31
**基于代码**: DragonOS master branch
