# RootFS Manifest Configuration (Docker Image / DADK) {#_dadk_rootfs_manifest}

This document describes DragonOS's RootFS Manifest configuration workflow (used by `make run` / `make write_diskimage`), which builds a DragonOS RootFS directly from official or locally built Docker images.

::: info
If you use the Nix RootFS pipeline (`nix run .#rootfs-*`), see [From Software Packages to RootFS Image (Nix)](/userland/rootfs/diskgen.html).
:::

## 1. Unified Entry Point & Built-in Manifests

All RootFS manifests live under `config/rootfs-manifests/<name>.toml`. The repository currently ships with:

| Manifest | Path | Base Image | FS / Size | Use Case |
| --- | --- | --- | --- | --- |
| `ubuntu2404` | `config/rootfs-manifests/ubuntu2404.toml` | `ubuntu:24.04` | `ext4` / `2G` | **Default on `x86_64`**; supports installing packages directly via `apt` |
| `docker` | `config/rootfs-manifests/docker.toml` | `dragonos-docker:ubuntu2404-29.1.3` | `ext4` / `4G` | Full environment for running Docker inside DragonOS (built from `tools/docker-rootfs/Dockerfile`) |
| `default` | `config/rootfs-manifests/default.toml` | None (empty) | `fat32` / `2G` | Default on non-`x86_64` architectures; minimal userspace from `sets/default` |

Select a manifest via the `ROOTFS_MANIFEST` environment variable:

```bash
ROOTFS_MANIFEST=ubuntu2404 make run-nographic
ROOTFS_MANIFEST=docker make run-nographic
ROOTFS_MANIFEST=default make run-nographic
```

When omitted, `env.mk` defaults to `ubuntu2404` on `x86_64` and `default` on other architectures.

## 2. Manifest Fields

Example (`config/rootfs-manifests/ubuntu2404.toml`):

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

Field reference:

- **`[metadata]`**
  - `name`: Manifest name.
  - `arch`: Target architecture (e.g., `x86_64`). Must match the current `ARCH` environment variable or the build will fail; `"*"` follows the current `ARCH`.
- **`[rootfs]`**
  - `fs_type`: Root filesystem type, typically `ext4` or `fat32`.
  - `size`: Disk image size (e.g., `2G`, `4G`). Increase this if your custom image is larger.
  - `partition`: Partition scheme, currently `mbr`.
- **`[base]`**
  - `image`: Docker image used as the RootFS base layer (remote or locally built via `docker build`); an empty string `""` means no base image.
  - `pull_policy`: Image pull policy, defaulting to `if-not-present` (uses the local image when already present).
- **`[user]`**
  - `config_dir`: DADK userspace package config directory overlaid on top of the base image (e.g., `user/dadk/config/sets/ubuntu2404`).

## 3. Adding Custom Files & Apps with a Local Docker Image

The most straightforward way to include your own binaries, libraries, or arbitrary files in DragonOS is to build a local Docker image and reference it in a manifest:

1. **Write a `Dockerfile` and build locally**

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

2. **Create or edit a manifest**

   Copy `config/rootfs-manifests/ubuntu2404.toml` to `config/rootfs-manifests/custom.toml`, then update `name`, `[base].image` (and `rootfs.size` if needed):

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

3. **Write the disk image or boot**

   ```bash
   # Regenerate disk image only
   ROOTFS_MANIFEST=custom SKIP_GRUB=1 make write_diskimage

   # Build and boot DragonOS
   ROOTFS_MANIFEST=custom make run-nographic
   ```

## 4. DADK Package Set Directory Layout

In addition to the Docker base image, repository-maintained utilities and tests are overlaid by DADK:

- `user/dadk/config/all/`: All application `.toml` definitions.
- `user/dadk/config/sets/<set-name>/`: Scenario-specific package sets (symlinks to `../../all/*.toml`), such as `sets/default/` and `sets/ubuntu2404/`.

Switch package sets by changing `user.config_dir` in the manifest.

## 5. Resolution & Automatic Reinstall

Before building, `tools/rootfs_manifest_resolve.sh` automatically generates:

- `config/rootfs.generated.toml`
- `dadk-manifest.generated.toml`

You can also validate resolution manually:

```bash
ROOTFS_MANIFEST=ubuntu2404 ARCH=x86_64 make prepare_rootfs_manifest
```

`user/Makefile` tracks manifest hashes automatically: switching `ROOTFS_MANIFEST` or editing a manifest cleans stale DADK output caches and rebuilds `bin/sysroot`, so manual cleanup is usually unnecessary.

## 6. Common Commands

```bash
# Full build and boot (graphical / serial console)
ROOTFS_MANIFEST=ubuntu2404 make run
ROOTFS_MANIFEST=ubuntu2404 make run-nographic

# Build disk image only (skip GRUB installation)
ROOTFS_MANIFEST=ubuntu2404 SKIP_GRUB=1 make write_diskimage

# Build and install userspace into bin/sysroot only
ROOTFS_MANIFEST=ubuntu2404 make user
```
