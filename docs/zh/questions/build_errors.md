# 构建错误常见问题解答

本文汇总在构建和运行 DragonOS 时遇到的常见问题及其解决方法。

## 1. 报错信息指示工具链缺失

### 问题现象

构建过程中出现 `xxxx not found` 或 Rust 工具链版本不匹配等报错。

### 解决方法

如果此前可以正常编译，拉取最新代码后出现此类报错，通常是因为上游更新了工具链或构建依赖要求。推荐重新运行一键初始化脚本更新环境：

```shell
cd tools
bash bootstrap.sh
```

完成后重新打开终端，再执行构建命令。`bootstrap.sh` 支持重复运行，会自动补齐最新所需的工具链。

## 2. 磁盘镜像写入失败或内容异常

### 问题现象

- 磁盘镜像中的用户态文件与预期不一致。
- 写盘时报错符号链接冲突（例如某个目录在新版基础镜像中变为了符号链接）。
- 部分应用程序未正确安装到镜像中。

### 解决方法

1. 确认当前磁盘剩余空间和权限充足。
2. 如切换了 `ROOTFS_MANIFEST` 或修改了自定义镜像，可先执行 `make prepare_rootfs_manifest` 校验配置。
3. 若旧的构建产物仍有残留冲突，直接清理 `bin/` 目录后重新构建通常可以解决大部分问题：

```shell
rm -rf bin/
make write_diskimage
```

## 3. RootFS Manifest 架构不匹配或镜像拉取失败

### 问题现象

- 报错 `Error: ARCH mismatch, env ARCH=..., rootfs manifest arch=...`。
- 构建时拉取 `ubuntu:24.04` 等 Docker 基础镜像超时或失败。

### 解决方法

- **架构不匹配**：`ubuntu2404` 和 `docker` 清单目前仅针对 `x86_64`。切换到 `riscv64` 或 `loongarch64` 时，请使用 `ROOTFS_MANIFEST=default`（或将自定义清单的 `metadata.arch` 设为目标架构）。
- **镜像拉取失败**：确认本机 Docker 服务已启动且当前用户具备 `docker` 权限；若网络受限，可预先在本地 `docker pull` 或 `docker build` 对应镜像，默认 `pull_policy = "if-not-present"` 会直接使用本地已有镜像。
