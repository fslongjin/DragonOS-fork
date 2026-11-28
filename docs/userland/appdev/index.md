# Application Development & Deployment Guide

DragonOS provides solid **Linux binary (ABI) compatibility**, supporting both dynamic and static linking across `glibc` and `musl` userspace environments.

## 1. Build & Run: No Special Configuration Needed

Developing or running applications on DragonOS **requires no special project templates, no mandatory static linking, and no custom compiler flags**:

- **Standard Toolchains**: Whether you are using C/C++ (`gcc` / `g++` / `clang`), Rust (`cargo build`), Go, Python, or other standard Linux software, simply compile as you normally would on Linux and run directly on DragonOS.
- **Dynamic Linking & Shared Libraries**: Dynamic linking is supported. As long as the target architecture matches DragonOS (e.g., `x86_64`) and required shared libraries are present in the RootFS, binaries run out of the box. Static linking also continues to work if you prefer self-contained binaries.
- **Mainstream Software Ready**: Common Linux CLI tools, language runtimes, and server software such as MySQL, ClickHouse, and Redis can run directly without modification.

## 2. Installing & Deploying to DragonOS

Depending on your workflow, choose one of the following three approaches to deploy your applications or files into DragonOS.

### Option 1: Local Docker Image + RootFS Manifest (Recommended)

This is the simplest and most flexible way to package custom binaries, configuration files, shared libraries, or a full runtime environment into a DragonOS RootFS.

#### Step 1: Write a Dockerfile and Build Locally

Start from `ubuntu:24.04` (or another compatible Linux base image) and copy in your binaries and dependencies:

```dockerfile
FROM ubuntu:24.04

# Install runtime dependencies or utilities as needed
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

# Copy your locally built binary or custom files
COPY ./my-app /usr/local/bin/my-app
RUN chmod +x /usr/local/bin/my-app
```

Build the image locally:

```bash
docker build -t my-dragonos-rootfs:latest .
```

> See `tools/docker-rootfs/Dockerfile` and `config/rootfs-manifests/docker.toml` in the repository for a complete real-world example.

#### Step 2: Configure a RootFS Manifest

Create a manifest file under `config/rootfs-manifests/` (for example, `config/rootfs-manifests/my-app.toml`) and point `[base].image` to your local image:

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

#### Step 3: Build the Disk Image and Boot DragonOS

Pass your manifest name via `ROOTFS_MANIFEST`:

```bash
# Generate disk image only
ROOTFS_MANIFEST=my-app SKIP_GRUB=1 make write_diskimage

# Or build and boot DragonOS (serial console mode)
ROOTFS_MANIFEST=my-app make run-nographic
```

For full manifest field documentation, image sizing, and automatic reinstall behavior, see [RootFS Manifest Configuration](/userland/rootfs/dadk-rootfs-manifest.html#_dadk_rootfs_manifest).

### Option 2: Install Directly via `apt` Inside DragonOS

On `x86_64`, DragonOS uses **Ubuntu 24.04** as the default RootFS (`ROOTFS_MANIFEST=ubuntu2404`). Once booted into DragonOS, you can install packages and runtime libraries directly with `apt`:

```bash
apt update
apt install -y <package-name>
```

### Option 3: Integrate Into the Repository Build System (For Contributors)

If you want to permanently bundle an application into the DragonOS repository and build it alongside the system:

- **Non-Nix (DADK) pipeline**: Add an application config under `user/dadk/config/` and link it into the appropriate `sets/` directory. See the [DADK Documentation](https://docs.dragonos.org.cn/p/dadk/) and [RootFS Manifest Configuration](/userland/rootfs/dadk-rootfs-manifest.html#_dadk_rootfs_manifest).
- **Nix pipeline**: Add a Nix derivation under `user/apps/default.nix`. See [Adding Built-in Programs in Nix](/userland/rootfs/add-your-own-app.html).
