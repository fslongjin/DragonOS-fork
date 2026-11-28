# Kernel Compilation Configuration Guide

DragonOS kernel compilation is configured through **Cargo Features** and **Makefile / `env.mk` environment variables**. The legacy `kernel.config` / `d.config` mechanism is no longer used.

## Cargo Features Configuration

Compile-time feature flags for the kernel are defined under the `[features]` section of `kernel/Cargo.toml`.

### Default Features

| Feature | Description |
| --- | --- |
| `fatfs` | Enables FAT filesystem support |
| `fatfs-secure` | Enables FAT security-related options (depends on `fatfs`) |
| `kvm` | Enables basic in-kernel KVM virtualization support |
| `kstack_protect` | Enables kernel stack protection to help detect stack overflows (doubles kernel stack memory usage) |
| `static_keys_test` | Enables static keys test support |

### Optional & Test Features

The following features are disabled by default and can be enabled on demand:

| Feature | Category | Description |
| --- | --- | --- |
| `initram` | Optional | Enables initramfs support (used together with `INITRAM_PATH`) |
| `driver_ps2_mouse` | Optional | Enables the PS/2 mouse driver |
| `mm_selftest` | Self-test | Controlled MM/DMA stress tests and fault-injection self-tests |
| `sched_selftest` | Self-test | Scheduler group-selection and CPU affinity regression self-tests |
| `cred_selftest` | Self-test | Process credential (`cred`) scope regression self-tests |
| `kprobe_test` | Self-test | kprobe functional tests |
| `fifo_demo` | Debug Demo | Per-CPU FIFO real-time runqueue smoke test |

### Enabling or Modifying Features

- **Append features at build time (Recommended)**: Pass extra features via the `KERNEL_FEATURES` environment variable (comma-separated). The build system forwards them to `cargo build --features`:

  ```bash
  # Enable a single self-test feature
  make kernel KERNEL_FEATURES=mm_selftest

  # Enable multiple features at once
  make kernel KERNEL_FEATURES="mm_selftest,sched_selftest"
  ```

- **Modify default features**: To persistently enable or disable a default feature (such as `kstack_protect`), edit the `default` list in `kernel/Cargo.toml` directly.

## Common Build Environment Variables

In addition to Cargo features, you can customize the build via command-line variables or by editing `env.mk` in the repository root:

| Variable | Default | Description |
| --- | --- | --- |
| `ARCH` | `x86_64` | Target architecture: `x86_64`, `riscv64`, or `loongarch64` (see [Target Architecture Configuration](/kernel/configuration/arch.html)) |
| `KERNEL_FEATURES` | Empty | Additional kernel Cargo features to enable (comma-separated) |
| `ROOTFS_MANIFEST` | `ubuntu2404` (`x86_64`)<br>`default` (other archs) | RootFS manifest name under `config/rootfs-manifests/` |
| `UNWIND_ENABLE` | `yes` (`no` on `loongarch64`) | Whether to enable kernel stack unwinding (`.eh_frame` and `panic=unwind`) |
| `INITRAM_PATH` | `$(ROOT_PATH)/x86.cpio.xz` | Path to the initramfs image, used with the `initram` feature |
| `SKIP_GRUB` | `0` (`1` in CI) | Set to `1` to skip automatic GRUB installation, useful in CI or nographic/serial runs |

### Usage Examples

```bash
# Build the kernel with default configuration
make kernel

# Build with kernel stack unwinding disabled
make kernel UNWIND_ENABLE=no

# Build for a specific architecture with an extra feature
make kernel ARCH=riscv64 KERNEL_FEATURES=kprobe_test
```
