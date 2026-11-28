# RootFS Build & Configuration

DragonOS supports two RootFS build pipelines depending on your development workflow:

- **Docker Image + RootFS Manifest (Recommended, `make` / DADK pipeline)**: Specify a base Docker image (such as `ubuntu:24.04`, a Docker-in-DragonOS image, or a locally built custom image) in `config/rootfs-manifests/<name>.toml` to generate a disk image. See [RootFS Manifest Configuration](/userland/rootfs/dadk-rootfs-manifest.html#_dadk_rootfs_manifest).
- **Nix Build Pipeline (`nix run .#yolo-*` / `nix run .#rootfs-*`)**: Manage built-in userspace packages as Nix derivations to produce reproducible RootFS images. See [From Packages to RootFS Image (Nix)](/userland/rootfs/diskgen.html) and [Adding Built-in Programs in Nix](/userland/rootfs/add-your-own-app.html).
