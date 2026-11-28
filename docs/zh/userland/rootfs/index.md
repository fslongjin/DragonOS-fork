# RootFS 构建与配置

DragonOS 支持两种 RootFS 构建链路，请按你的开发方式选择：

- **Docker 镜像 + RootFS Manifest（推荐，对应 `make` / DADK 链路）**：通过 `config/rootfs-manifests/<name>.toml` 指定基础 Docker 镜像（如 `ubuntu:24.04`、Docker 运行环境镜像或本地 `docker build` 的自定义镜像）并生成磁盘镜像。详见 [RootFS Manifest 配置](/zh/userland/rootfs/dadk-rootfs-manifest.html#_dadk_rootfs_manifest)。
- **Nix 构建链路（对应 `nix run .#yolo-*` / `nix run .#rootfs-*`）**：通过 Nix derivation 管理内置用户态软件包并生成可复现的 RootFS 镜像。详见 [从软件包到 RootFS 镜像（Nix）](/zh/userland/rootfs/diskgen.html) 和 [在 Nix 中添加内置程序](/zh/userland/rootfs/add-your-own-app.html)。
