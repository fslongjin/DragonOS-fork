# Common Build Issues

This page summarizes common issues encountered when building and running DragonOS and how to resolve them.

## 1. Missing Toolchain or Version Mismatch

### Symptom

Build fails with `xxxx not found` or a Rust toolchain version mismatch error.

### Solution

If builds previously succeeded and broke after pulling the latest commits, upstream toolchain requirements have likely been updated. Re-run the bootstrap script to update your environment:

```shell
cd tools
bash bootstrap.sh
```

Open a new terminal and re-run the build. `bootstrap.sh` is designed to be idempotent and safe to re-run at any time.

## 2. Disk Image Write Failure or Stale Contents

### Symptom

- Userspace files inside the disk image do not match expectations.
- Symlink conflict errors occur during disk image creation (for example, a directory became a symlink in a newer base image).
- Certain applications are missing from the generated image.

### Solution

1. Check that sufficient disk space and permissions are available.
2. If you switched `ROOTFS_MANIFEST` or updated a custom image, run `make prepare_rootfs_manifest` to verify the resolved configuration.
3. If stale build artifacts remain, removing `bin/` and rebuilding resolves most issues:

```shell
rm -rf bin/
make write_diskimage
```

## 3. RootFS Manifest Architecture Mismatch or Image Pull Failure

### Symptom

- `Error: ARCH mismatch, env ARCH=..., rootfs manifest arch=...`
- Pulling a base Docker image such as `ubuntu:24.04` times out or fails.

### Solution

- **Architecture mismatch**: The `ubuntu2404` and `docker` manifests target `x86_64`. When building for `riscv64` or `loongarch64`, use `ROOTFS_MANIFEST=default` (or set `metadata.arch` in your custom manifest accordingly).
- **Image pull failure**: Verify that the Docker daemon is running and your user has permission to run `docker`. In restricted networks, pre-pull or locally `docker build` the base image—the default `pull_policy = "if-not-present"` uses local images directly.
