# 加载程序

## 1. 二进制程序与脚本装载

本节简要介绍 DragonOS 中 `execve` 路径的二进制与脚本加载机制（实现位于 `kernel/src/process/exec.rs`、`kernel/src/process/shebang.rs` 和 `kernel/src/libs/elf.rs`）。

DragonOS 在执行程序时，采用 **「探测 — 装载」** 的流程：

1. **脚本解释器探测（Shebang）**：首先检查文件首部是否以 `#!` 开头。如果是脚本文件，Shebang 加载器会解析解释器路径与可选参数，并递归重新触发目标解释器的装载（受最大递归深度限制 `SHEBANG_MAX_RECURSION_DEPTH` 保护）。
2. **二进制格式探测（Probe）**：读取文件头并依次调用已注册的 `BinaryLoader` 的 `probe` 方法，判断是否支持当前可执行文件格式。
3. **装载与映射（Load）**：使用匹配的加载器将程序段映射到新进程的用户地址空间，准备用户栈、参数（`argv`）、环境变量（`envp`）及辅助向量（`auxv`），并返回程序入口地址。

## 2. 当前支持的格式与链接方式

- **ELF 加载器（`ELF_LOADER`）**：支持标准 64 位 ELF 可执行文件，同时支持 **静态链接** 与 **动态链接**（解析 `PT_INTERP` 装载动态链接器，如 `glibc` / `musl` 的 `ld.so`，并通过 `auxv` 传递 `AT_PHDR`、`AT_ENTRY`、`AT_BASE` 等信息）以及 PIE 可执行文件。
- **Shebang 加载器（`SHEBANG_LOADER`）**：支持 `#!/bin/sh`、`#!/usr/bin/env python3` 等常见脚本可执行文件。
