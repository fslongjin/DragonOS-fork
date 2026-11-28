# 应用开发与部署指南

DragonOS 提供良好的 **Linux 二进制（ABI）兼容性**，支持动态链接与静态链接，兼容 `glibc` 与 `musl` 用户态环境。

## 1. 编译与运行：无需特殊配置

在 DragonOS 上开发或运行软件，**不需要专用项目模板，也不需要强制静态链接或特殊编译参数**：

- **通用语言与工具链**：无论是 C/C++（`gcc` / `g++` / `clang`）、Rust（`cargo build`）、Go、Python，还是其他标准 Linux 软件，直接按照常规 Linux 方式编译构建即可在 DragonOS 上运行。
- **动态链接与依赖库**：DragonOS 已支持动态链接。只要目标架构与 DragonOS 一致（例如 `x86_64`），且程序所需的共享库存在于 RootFS 中，即可直接执行；你也可以继续选择静态链接以减少运行环境依赖。
- **主流软件开箱即用**：常见 Linux 命令行工具、编程语言运行时以及 MySQL、ClickHouse、Redis 等主流服务端软件均可直接运行。

## 2. 安装与部署到 DragonOS

根据你的使用场景，可以选择以下三种方式将自己的程序或文件部署到 DragonOS 中。

### 方式一：本地构建 Docker 镜像 + 修改 RootFS Manifest（推荐）

这是最简单、最通用的方式，适合将自定义二进制、配置文件、依赖库或完整运行环境打包进 DragonOS RootFS。

#### 步骤 1：编写 Dockerfile 并构建本地镜像

基于 `ubuntu:24.04`（或其他兼容的 Linux 基础镜像）放入你的程序和依赖：

```dockerfile
FROM ubuntu:24.04

# 按需安装运行时依赖或常用工具
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

# 拷贝本地编译好的程序或自定义文件
COPY ./my-app /usr/local/bin/my-app
RUN chmod +x /usr/local/bin/my-app
```

在本地构建镜像：

```bash
docker build -t my-dragonos-rootfs:latest .
```

> 仓库中的 `tools/docker-rootfs/Dockerfile` 与 `config/rootfs-manifests/docker.toml` 提供了一个完整的实战示例，可直接参考。

#### 步骤 2：配置 RootFS Manifest

在 `config/rootfs-manifests/` 目录下新建一个清单文件（例如 `config/rootfs-manifests/my-app.toml`），将 `[base]` 中的 `image` 指向刚构建的本地镜像：

```toml
[metadata]
name = "my-app"
arch = "x86_64"

[rootfs]
fs_type = "ext4"
size = "2G"
partition = "mbr"

[base]
image = "my-dragonos-rootfs:latest"
pull_policy = "if-not-present"

[user]
config_dir = "user/dadk/config/sets/ubuntu2404"
```

#### 步骤 3：生成镜像并启动 DragonOS

通过 `ROOTFS_MANIFEST` 指定刚才创建的清单名称即可：

```bash
# 仅生成磁盘镜像
ROOTFS_MANIFEST=my-app SKIP_GRUB=1 make write_diskimage

# 或直接构建并启动 DragonOS（串口模式）
ROOTFS_MANIFEST=my-app make run-nographic
```

关于 Manifest 各字段语义、镜像大小调整及缓存重装机制，请参见 [RootFS Manifest 配置](/zh/userland/rootfs/dadk-rootfs-manifest.html#_dadk_rootfs_manifest)。

### 方式二：在 DragonOS 内直接使用 `apt` 安装

`x86_64` 架构默认采用 **Ubuntu 24.04** 作为 RootFS（`ROOTFS_MANIFEST=ubuntu2404`）。启动进入 DragonOS 后，可以直接使用 `apt` 安装现成软件包或补充运行库：

```bash
apt update
apt install -y <package-name>
```

### 方式三：将应用集成到仓库构建体系（面向贡献者）

如果你希望将某个应用长期纳入 DragonOS 仓库并随项目一同构建，可按所使用的构建链路参考：

- **非 Nix（DADK）链路**：在 `user/dadk/config/` 中添加应用配置并挂入对应的 `sets/` 集合，详见 [DADK 官方文档](https://docs.dragonos.org.cn/p/dadk/) 与 [RootFS Manifest 配置](/zh/userland/rootfs/dadk-rootfs-manifest.html#_dadk_rootfs_manifest)。
- **Nix 链路**：在 `user/apps/default.nix` 中添加 Nix derivation，详见 [在 Nix 中添加内置程序](/zh/userland/rootfs/add-your-own-app.html)。
