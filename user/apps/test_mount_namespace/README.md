# Mount Namespace Test

这是一个用于测试DragonOS Mount Namespace功能的程序。

## 功能

- 测试mount namespace的创建和隔离
- 验证unshare(CLONE_NEWNS)系统调用
- 测试在新namespace中挂载文件系统
- 验证文件系统的隔离性
- 检查/proc/mounts显示

## 运行

```bash
/bin/test_mount_namespace
```

## 预期输出

程序将创建子进程，在子进程中创建新的mount namespace，然后挂载tmpfs/ramfs到测试目录，验证父子进程间的文件系统隔离。