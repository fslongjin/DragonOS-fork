# 目标架构配置

## 支持的架构

DragonOS 目前支持以下 64 位架构：

- **`x86_64`**（默认、主力支持架构）
- **`riscv64`**
- **`loongarch64`**

## 切换目标架构

### 1. 命令行临时指定（推荐）

构建时可直接通过 `ARCH` 环境变量指定目标架构：

```bash
make kernel ARCH=x86_64
make kernel ARCH=riscv64
make kernel ARCH=loongarch64
```

### 2. 修改默认架构配置

如需长期切换默认架构，可编辑仓库根目录的 `env.mk`：

```makefile
ifeq ($(ARCH), )
# 可选: x86_64, riscv64, loongarch64
export ARCH?=x86_64
endif
```

同时，如果使用 VS Code + `rust-analyzer` 开发内核，请同步更新 `.vscode/settings.json` 中的 `rust-analyzer.cargo.target`，使代码补全与静态检查匹配当前目标架构：

- `x86_64`：`kernel/src/arch/x86_64/x86_64-unknown-none.json`（或 `x86_64-unknown-none`）
- `riscv64`：`kernel/src/arch/riscv64/riscv64gc-unknown-none-elf.json`（或 `riscv64gc-unknown-none-elf`）
- `loongarch64`：`kernel/src/arch/loongarch64/loongarch64-unknown-none.json`

## 注意事项

- 不同架构的默认 RootFS 清单不同：`x86_64` 默认使用 `ROOTFS_MANIFEST=ubuntu2404`，其他架构默认使用 `ROOTFS_MANIFEST=default`（`config/rootfs-manifests/`）。
- 切换架构后请先执行 `make clean` 清理旧的构建产物，再重新构建运行。
