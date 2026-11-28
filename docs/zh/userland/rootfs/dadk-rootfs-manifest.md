# RootFS Manifest 配置（Docker 镜像 / DADK） {#_dadk_rootfs_manifest}

本文说明 DragonOS 基于 RootFS Manifest 的镜像配置方式（对应 `make run` / `make write_diskimage` 构建链路），支持直接基于官方或本地构建的 Docker 镜像生成 DragonOS RootFS。

::: info
如果你使用的是 Nix RootFS 构建链路（`nix run .#rootfs-*`），请参考 [从软件包到 RootFS 镜像（Nix）](/zh/userland/rootfs/diskgen.html)。
:::

## 1. 统一入口与内置 Manifest

所有 RootFS 清单统一放置在 `config/rootfs-manifests/<name>.toml`，仓库当前内置了以下清单：

| 清单名 | 文件路径 | 基础镜像 | 文件系统 / 大小 | 适用场景 |
| --- | --- | --- | --- | --- |
| `ubuntu2404` | `config/rootfs-manifests/ubuntu2404.toml` | `ubuntu:24.04` | `ext4` / `2G` | **`x86_64` 默认**，支持直接 `apt` 安装软件 |
| `docker` | `config/rootfs-manifests/docker.toml` | `dragonos-docker:ubuntu2404-29.1.3` | `ext4` / `4G` | 在 DragonOS 内运行 Docker 的完整环境（镜像由 `tools/docker-rootfs/Dockerfile` 构建） |
| `default` | `config/rootfs-manifests/default.toml` | 无（空） | `fat32` / `2G` | 非 `x86_64` 架构默认，仅包含 `sets/default` 下的最小用户态程序 |

通过环境变量 `ROOTFS_MANIFEST` 切换清单：

```bash
ROOTFS_MANIFEST=ubuntu2404 make run-nographic
ROOTFS_MANIFEST=docker make run-nographic
ROOTFS_MANIFEST=default make run-nographic
```

未显式指定时，`env.mk` 在 `x86_64` 架构下默认使用 `ubuntu2404`，其他架构默认使用 `default`。

## 2. Manifest 字段说明

以 `config/rootfs-manifests/ubuntu2404.toml` 为例：

```toml
[metadata]
name = "ubuntu2404"
arch = "x86_64"

[rootfs]
fs_type = "ext4"
size = "2G"
partition = "mbr"

[base]
image = "ubuntu:24.04"
pull_policy = "if-not-present"

[user]
config_dir = "user/dadk/config/sets/ubuntu2404"
```

各字段语义如下：

- **`[metadata]`**
  - `name`：清单名称。
  - `arch`：目标架构（如 `x86_64`），必须与当前 `ARCH` 环境变量一致，否则构建前会报错；设为 `"*"` 表示跟随当前 `ARCH`。
- **`[rootfs]`**
  - `fs_type`：根文件系统类型，常用 `ext4` 或 `fat32`。
  - `size`：磁盘镜像大小（如 `2G`、`4G`）。如果自定义镜像体积较大，可按需调大该值。
  - `partition`：分区表格式，当前常用 `mbr`。
- **`[base]`**
  - `image`：用作 RootFS 基础层的 Docker 镜像名（支持远程镜像或本地 `docker build` 构建的镜像）；留空 `""` 表示不使用基础镜像。
  - `pull_policy`：镜像拉取策略，默认为 `if-not-present`（本地已存在该镜像时直接使用本地镜像，不会重复拉取）。
- **`[user]`**
  - `config_dir`：附加安装的 DADK 用户态程序配置目录（如 `user/dadk/config/sets/ubuntu2404`）。

## 3. 使用自定义本地 Docker 镜像添加文件与应用

如果你想往 DragonOS 里添加自己的程序、依赖库或任意文件，最直接的方式是在本地构建一个 Docker 镜像，然后在 Manifest 中引用它：

1. **编写 `Dockerfile` 并本地构建镜像**

   ```dockerfile
   FROM ubuntu:24.04

   RUN apt-get update && apt-get install -y --no-install-recommends \
       ca-certificates \
       && rm -rf /var/lib/apt/lists/*

   COPY ./my-app /usr/local/bin/my-app
   COPY ./my-config.yaml /etc/my-app/config.yaml
   ```

   ```bash
   docker build -t my-dragonos-rootfs:latest .
   ```

2. **新增或修改 Manifest 文件**

   复制一份 `config/rootfs-manifests/ubuntu2404.toml` 为 `config/rootfs-manifests/custom.toml`，只需修改 `name`、`[base].image`（以及按需调整 `rootfs.size`）：

   ```toml
   [metadata]
   name = "custom"
   arch = "x86_64"

   [rootfs]
   fs_type = "ext4"
   size = "4G"
   partition = "mbr"

   [base]
   image = "my-dragonos-rootfs:latest"
   pull_policy = "if-not-present"

   [user]
   config_dir = "user/dadk/config/sets/ubuntu2404"
   ```

3. **写盘或启动运行**

   ```bash
   # 仅重新生成磁盘镜像
   ROOTFS_MANIFEST=custom SKIP_GRUB=1 make write_diskimage

   # 构建并启动 DragonOS
   ROOTFS_MANIFEST=custom make run-nographic
   ```

## 4. DADK 包集合目录约定

除了 Docker 基础镜像外，仓库内的部分系统工具与测试程序由 DADK 叠加安装到 RootFS 中：

- `user/dadk/config/all/`：存放所有应用的 `.toml` 配置。
- `user/dadk/config/sets/<set-name>/`：按场景组织的包集合（通过软链接引用 `../../all/*.toml`），例如 `sets/default/`、`sets/ubuntu2404/`。

通过修改 Manifest 中的 `user.config_dir` 即可切换附加安装的包集合。

## 5. 解析流程与自动重装机制

构建前系统会自动调用 `tools/rootfs_manifest_resolve.sh`，根据所选清单生成两份配置文件：

- `config/rootfs.generated.toml`
- `dadk-manifest.generated.toml`

也可以单独执行校验：

```bash
ROOTFS_MANIFEST=ubuntu2404 ARCH=x86_64 make prepare_rootfs_manifest
```

`user/Makefile` 内置了 Manifest 变更检测机制：当你切换 `ROOTFS_MANIFEST` 或修改清单内容后，系统会自动清理旧的 DADK 输出缓存并重建 `bin/sysroot`，避免旧文件污染新镜像，通常无需手动清理 `bin/sysroot`。

## 6. 常用命令速查

```bash
# 完整构建并启动（图形 / 串口模式）
ROOTFS_MANIFEST=ubuntu2404 make run
ROOTFS_MANIFEST=ubuntu2404 make run-nographic

# 仅生成磁盘镜像（跳过 GRUB 安装）
ROOTFS_MANIFEST=ubuntu2404 SKIP_GRUB=1 make write_diskimage

# 仅构建用户态程序并安装到 bin/sysroot
ROOTFS_MANIFEST=ubuntu2404 make user
```
