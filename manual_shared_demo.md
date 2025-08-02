# 共享子树手动演示指南

## 准备工作

首先准备两个终端窗口：
- **终端1（主namespace）**: 用于创建共享挂载和观察
- **终端2（新namespace）**: 用于测试传播效果

## 演示1：MS_SHARED（共享传播）

### 在终端1中执行：

```bash
# 1. 创建测试目录
mkdir -p /tmp/shared_demo

# 2. 创建并设置共享挂载
sudo mount -t tmpfs none /tmp/shared_demo
sudo mount --make-shared /tmp/shared_demo

# 3. 查看当前挂载状态
findmnt /tmp/shared_demo

# 4. 查看传播类型（注意shared:xxx标记）
grep "/tmp/shared_demo" /proc/self/mountinfo
```

### 在终端2中执行：

```bash
# 1. 创建新的mount namespace
sudo unshare --mount bash

# 2. 验证我们在新namespace中
echo "当前namespace ID: $(readlink /proc/self/ns/mnt)"

# 3. 检查是否能看到共享挂载
findmnt /tmp/shared_demo

# 4. 在共享挂载下创建子目录和挂载
mkdir -p /tmp/shared_demo/subdir
mount -t tmpfs none /tmp/shared_demo/subdir

# 5. 查看新namespace中的挂载状态
findmnt /tmp/shared_demo
```

### 回到终端1观察：

```bash
# 检查是否看到了从新namespace传播来的挂载
findmnt /tmp/shared_demo
# 你应该能看到 subdir 的挂载！这就是共享传播的效果
```

## 演示2：MS_PRIVATE（私有隔离）

### 在终端1中执行：

```bash
# 1. 创建私有挂载
mkdir -p /tmp/private_demo
sudo mount -t tmpfs none /tmp/private_demo
sudo mount --make-private /tmp/private_demo

# 2. 查看私有挂载（注意没有shared标记）
grep "/tmp/private_demo" /proc/self/mountinfo
```

### 在终端2中执行：

```bash
# 1. 在新namespace中创建子挂载
mkdir -p /tmp/private_demo/subdir
mount -t tmpfs none /tmp/private_demo/subdir

# 2. 查看新namespace中的状态
findmnt /tmp/private_demo
```

### 回到终端1观察：

```bash
# 检查私有挂载的状态
findmnt /tmp/private_demo
# 你会发现看不到subdir挂载！这就是私有挂载的隔离效果
```

## 演示3：MS_SLAVE（单向传播）

### 在终端1中执行：

```bash
# 1. 创建主共享挂载
mkdir -p /tmp/slave_demo
sudo mount -t tmpfs none /tmp/slave_demo
sudo mount --make-shared /tmp/slave_demo
```

### 在终端2中执行：

```bash
# 1. 将挂载设置为从属模式
mount --make-slave /tmp/slave_demo

# 2. 查看从属挂载的标记（注意master:xxx标记）
grep "/tmp/slave_demo" /proc/self/mountinfo
```

### 在终端1中执行：

```bash
# 1. 在主挂载下创建子挂载
mkdir -p /tmp/slave_demo/from_master
sudo mount -t tmpfs none /tmp/slave_demo/from_master

# 2. 查看主namespace的状态
findmnt /tmp/slave_demo
```

### 在终端2中观察：

```bash
# 检查从属namespace是否接收到了传播
findmnt /tmp/slave_demo
# 你应该能看到from_master挂载！

# 现在在从属namespace中创建挂载
mkdir -p /tmp/slave_demo/from_slave
mount -t tmpfs none /tmp/slave_demo/from_slave
```

### 回到终端1观察：

```bash
# 检查主namespace是否看到从属的挂载
findmnt /tmp/slave_demo
# 你会发现看不到from_slave挂载！这证明了单向传播特性
```

## 清理

```bash
# 在两个终端中都执行清理
sudo umount -R /tmp/shared_demo 2>/dev/null
sudo umount -R /tmp/private_demo 2>/dev/null  
sudo umount -R /tmp/slave_demo 2>/dev/null
sudo rm -rf /tmp/{shared_demo,private_demo,slave_demo}
```

## 关键观察点

1. **shared标记**: 在`/proc/self/mountinfo`中查看`shared:ID`
2. **master标记**: 从属挂载会显示`master:ID`
3. **传播方向**: 
   - SHARED: 双向传播
   - PRIVATE: 无传播
   - SLAVE: 单向接收传播
4. **namespace隔离**: 不同namespace有不同的挂载视图

## 实用命令

```bash
# 查看所有挂载的传播类型
findmnt -o TARGET,PROPAGATION

# 查看特定挂载的详细信息
findmnt --verbose /path/to/mount

# 查看mountinfo中的传播标记
cat /proc/self/mountinfo | grep -E "shared|master|propagate"
``` 