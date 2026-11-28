# 内核编译配置说明

DragonOS 的内核编译配置基于 **Cargo Features** 与 **Makefile / `env.mk` 环境变量** 共同管理，不再使用早期的 `kernel.config` / `d.config` 机制。

## Cargo Features 配置

内核的编译期功能开关定义在 `kernel/Cargo.toml` 的 `[features]` 段中。

### 默认启用的 Features

| Feature | 说明 |
| --- | --- |
| `fatfs` | 启用 FAT 文件系统支持 |
| `fatfs-secure` | 启用 FAT 安全相关选项（依赖 `fatfs`） |
| `kvm` | 启用基础 KVM 虚拟化支持 |
| `kstack_protect` | 启用内核栈保护以辅助检测栈溢出（会使内核栈内存占用翻倍） |
| `static_keys_test` | 启用 static keys 测试支持 |

### 可选功能与测试 Features

以下 Feature 默认关闭，可根据需要按需开启：

| Feature | 类别 | 说明 |
| --- | --- | --- |
| `initram` | 可选功能 | 启用 initramfs 支持（配合 `INITRAM_PATH` 使用） |
| `driver_ps2_mouse` | 可选功能 | 启用 PS/2 鼠标驱动 |
| `mm_selftest` | 内核自测 | 内存管理与 DMA 压测及受控故障注入自测 |
| `sched_selftest` | 内核自测 | 调度器组调度与 CPU 亲和性回归自测 |
| `cred_selftest` | 内核自测 | 进程凭证（cred）作用域回归自测 |
| `kprobe_test` | 内核自测 | kprobe 功能测试 |
| `fifo_demo` | 调试演示 | 每 CPU FIFO 实时调度队列烟雾测试 |

### 如何启用或修改 Features

- **构建时临时追加（推荐）**：通过 `KERNEL_FEATURES` 环境变量传入（多个 Feature 用逗号分隔），构建脚本会将其透传给 `cargo build --features`：

  ```bash
  # 开启单个自测 feature
  make kernel KERNEL_FEATURES=mm_selftest

  # 同时开启多个 feature
  make kernel KERNEL_FEATURES="mm_selftest,sched_selftest"
  ```

- **调整默认 Features**：如需长期开启或关闭某个默认 Feature（例如调整 `kstack_protect`），可直接修改 `kernel/Cargo.toml` 中的 `default` 列表。

## 常用构建环境变量

除 Cargo Features 外，还可通过命令行参数或修改根目录 `env.mk` 调整构建行为：

| 环境变量 | 默认值 | 说明 |
| --- | --- | --- |
| `ARCH` | `x86_64` | 目标架构，支持 `x86_64`、`riscv64`、`loongarch64`（详见 [目标架构配置](/zh/kernel/configuration/arch.html)） |
| `KERNEL_FEATURES` | 空 | 追加启用的内核 Cargo features，逗号分隔 |
| `ROOTFS_MANIFEST` | `ubuntu2404`（`x86_64`）<br>`default`（其他架构） | RootFS 清单名称，对应 `config/rootfs-manifests/` 下的配置 |
| `UNWIND_ENABLE` | `yes`（`loongarch64` 为 `no`） | 是否启用内核栈展开（`.eh_frame` 与 `panic=unwind`） |
| `INITRAM_PATH` | `$(ROOT_PATH)/x86.cpio.xz` | initramfs 镜像路径，配合 `initram` feature 使用 |
| `SKIP_GRUB` | `0`（CI 环境为 `1`） | 设为 `1` 时跳过 GRUB 自动安装，适用于 CI 或纯串口/nographic 场景 |

### 常见用法示例

```bash
# 使用默认配置编译内核
make kernel

# 关闭内核栈展开编译
make kernel UNWIND_ENABLE=no

# 指定目标架构与额外 feature 编译
make kernel ARCH=riscv64 KERNEL_FEATURES=kprobe_test
```
