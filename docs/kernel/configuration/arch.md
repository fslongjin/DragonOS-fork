# Target Architecture Configuration

## Supported Architectures

DragonOS currently supports the following 64-bit architectures:

- **`x86_64`** (default, primary supported architecture)
- **`riscv64`**
- **`loongarch64`**

## Switching Target Architecture

### 1. Override via Command Line (Recommended)

Specify the target architecture directly with the `ARCH` environment variable when building:

```bash
make kernel ARCH=x86_64
make kernel ARCH=riscv64
make kernel ARCH=loongarch64
```

### 2. Change the Default Architecture

To persistently change the default architecture, edit `env.mk` in the repository root:

```makefile
ifeq ($(ARCH), )
# Supported: x86_64, riscv64, loongarch64
export ARCH?=x86_64
endif
```

If you develop the kernel with VS Code and `rust-analyzer`, also update `rust-analyzer.cargo.target` in `.vscode/settings.json` so code completion and diagnostics match your chosen architecture:

- `x86_64`: `kernel/src/arch/x86_64/x86_64-unknown-none.json` (or `x86_64-unknown-none`)
- `riscv64`: `kernel/src/arch/riscv64/riscv64gc-unknown-none-elf.json` (or `riscv64gc-unknown-none-elf`)
- `loongarch64`: `kernel/src/arch/loongarch64/loongarch64-unknown-none.json`

## Notes

- Default RootFS manifests differ by architecture: `x86_64` defaults to `ROOTFS_MANIFEST=ubuntu2404`, while other architectures default to `ROOTFS_MANIFEST=default` (see `config/rootfs-manifests/`).
- After switching architectures, run `make clean` to clear previous build artifacts before rebuilding.
