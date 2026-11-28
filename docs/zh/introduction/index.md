# DragonOS 简介

DragonOS 是一个面向 **Agent Native** 时代的轻量级 64 位独立内核操作系统，使用 Rust 语言从零构建，提供 **Linux 二进制（ABI）兼容性**。它专为高效、安全地运行 **AI Agent 沙箱（Sandbox）** 与 **云原生服务（Service）** 而设计。

## 为什么是 Agent Native？

在 AI Agent 广泛执行代码、调用外部工具、编排复杂工作流的时代，底层操作系统正面临新的基础设施需求：

- **高密度、轻量化的 Agent 沙箱**：Agent 运行时通常需要频繁创建和销毁隔离环境，对极速启动、低内存占用与小攻击面提出了极高要求。作为轻量级 **Guest OS**，DragonOS 摆脱了传统通用内核的历史包袱，借助 Rust 的内存安全特性与精简的内核架构，配合命名空间（Namespace）与资源控制（Cgroup）等机制，在沙箱内部提供高效、可靠的进程隔离与资源管控，适合大规模、高密度的 Agent 执行场景。
- **丰富的语言运行时与主流服务生态**：围绕 Agent 的代码执行、工具服务、数据存储与长期运行的后端服务，高度依赖成熟的 Linux 生态。DragonOS 坚持 Linux 二进制兼容路线，并持续完善容器支持、网络协议栈、调度子系统与 eBPF 等能力，不仅支持 C/C++、Go、Python、Rust 等主流编程语言及运行时环境，像 MySQL、ClickHouse 等常见主流服务端软件也能够在无需修改的情况下直接在 DragonOS 上运行。

## 技术特点与演进

- **Rust 独立内核**：使用 Rust 编写核心子系统，从语言层面显著降低内存安全漏洞风险，提升系统在多租户与高并发场景下的稳定性。
- **面向云环境的精简 Guest 架构**：专为轻量化云计算与 Guest OS 运行环境设计，在设备模型（如 VirtIO）、调度、网络与隔离机制上保持简洁高效，并持续推进云平台适配与多架构（如 RISC-V）支持。
- **持续演进的 Linux 兼容性**：DragonOS 正在持续扩展系统调用与内核接口覆盖率，不断提升对生产级 Linux 应用和容器工作负载的兼容能力。你可以在 [DragonOS CI Dashboard](https://ci-dashboard.dragonos.org) 查看最新的兼容性测试进展。

## 开源与协议

DragonOS 开源项目始于 2022 年 7 月，采用 **GPLv2** 协议开放源代码。我们欢迎对操作系统内核、Agent 基础设施与云原生技术感兴趣的开发者和团队共同参与建设。

如果你想了解 DragonOS 已经实现的功能模块，可以继续阅读：[功能特性](/zh/introduction/features.html#_genreal_features)。
