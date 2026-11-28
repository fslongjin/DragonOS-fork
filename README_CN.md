<div align="center">
  <img width="40%" src="docs/_static/dragonos-logo.svg" alt="dragonos-logo"><br/>
  <h2>面向 Agent Native 时代的轻量级 Rust 操作系统内核</h2>

<a href="https://dragonos.org"><img alt="官网" src="https://img.shields.io/badge/%E5%AE%98%E7%BD%91-DragonOS.org-4c69e4"></a>
<a href="https://docs.dragonos.org"><img alt="文档" src="https://img.shields.io/badge/%E6%96%87%E6%A1%A3-docs.dragonos.org-2ea44f"></a>
<a href="https://ci-dashboard.dragonos.org"><img alt="CI Dashboard" src="https://img.shields.io/badge/CI_Dashboard-Linux_%E5%85%BC%E5%AE%B9%E6%80%A7-orange"></a>
<a href="https://bbs.dragonos.org.cn"><img alt="BBS" src="https://img.shields.io/badge/BBS-bbs.dragonos.org.cn-purple"></a>
<a href="./LICENSE"><img alt="License: GPLv2" src="https://img.shields.io/badge/License-GPLv2-blue"></a>

---

</div>

# DragonOS

**Languages**: 中文 | [English](README.md)

DragonOS 是一个面向 **Agent Native** 时代的轻量级 64 位独立内核操作系统，使用 Rust 语言从零构建，提供 **Linux 二进制（ABI）兼容性**。作为轻量化 **Guest OS**，它摆脱了传统通用内核的历史包袱，专为高效、安全地运行 **AI Agent 沙箱（Sandbox）** 与 **云原生服务（Service）** 而设计。

🌟 **想要快速体验 DragonOS？** 访问 [DragonOS Playground](https://cnb.cool/DragonOS-Community/playground) 即可在云端零配置一键启动 DragonOS，体验最新 nightly build！

## ✨ 核心亮点

- **Agent 沙箱与容器底座**：原生兼容 **Cube Sandbox**（高密度部署场景下，相比 Linux 可实现约 **10%** 的内存节省）与 **Docker**；内核完整支持 Linux Namespaces、`Cgroup v2` 与 `seccomp`。
- **高 Linux 兼容与开箱即用生态**：`x86_64` 默认采用 **Ubuntu 24.04** 作为 RootFS，可直接通过 `apt` 安装软件包；支持动态与静态链接（`glibc` / `musl`），支持 C/C++、Go、Python、Rust 等主流编程语言运行时，**MySQL、ClickHouse、Redis** 等常见主流服务端软件可在无需修改的情况下直接运行。
- **Rust 独立内核与现代子系统**：
  - **存储与文件系统**：统一 VFS、Page Cache、OverlayFS、VirtIO-FS、FUSE、Ext4、`tmpfs`、`procfs` / `sysfs` / `cgroup2`。
  - **网络与 Guest-Host 通信**：独立 IPv4 / IPv6 协议栈、NetNS、`veth`、软件网桥、`nftables` / `conntrack` 以及 **VirtIO-VSOCK**。
  - **可观测性与弹性能力**：支持 eBPF、`kprobe`、`uprobe`、`tracepoint`、`perf_event_open`，以及 `virtio-balloon`、OOM Killer / Reaper 等云原生 Guest 机制。

更多已实现功能请查看 [DragonOS 功能特性](https://docs.dragonos.org/zh/introduction/features.html#_genreal_features)，实时系统调用与 Linux 兼容性测试数据请访问 [DragonOS CI Dashboard](https://ci-dashboard.dragonos.org)。

## 📰 社区新闻

- **2025-12-22**: 🚀 **DragonOS 0.4.0 已发布！** 聚焦 Linux 语义对齐与容器/云原生基础能力，补强 I/O 多路复用、POSIX 定时器、`tmpfs`、挂载传播与 `procfs` 等核心特性。[查看更新日志 →](https://docs.dragonos.org/zh/community/ChangeLog/V0.4.x/V0.4.0.html)
- **2025-12-17**: ☁️ **DragonOS Playground 已上线！** 在 CNB 平台零配置一键体验 DragonOS，支持每日 nightly build 更新！[立即体验 →](https://cnb.cool/DragonOS-Community/playground)
- **2025-11-20**: 🚀 DragonOS 0.3.0 已发布！支持 Go 程序直接运行！
- **2025-11-18**: 📊 DragonOS CI Dashboard 已上线！[立刻查看 DragonOS 的 Linux 兼容性数据 →](https://ci-dashboard.dragonos.org)

## 🚀 如何运行？

### 🌟 方法一：云端零配置体验（Playground）

在 CNB 云原生开发平台上零配置一键体验 DragonOS，无需本地安装任何依赖：

<div align="center">

[![在CNB一键启动](docs/_static/cnb-launch-button-cn.svg)](https://cnb.cool/DragonOS-Community/playground)

</div>

### 方法二：本地构建与部署

本地开发推荐使用一键环境初始化脚本 `tools/bootstrap.sh`，也支持通过本地构建 Docker 镜像并配置 RootFS Manifest 来部署你自己的应用与文件：

- [构建 DragonOS — DragonOS 文档](https://docs.dragonos.org/zh/introduction/build_system.html)
- [应用开发与部署指南（Docker RootFS 定制）](https://docs.dragonos.org/zh/userland/appdev/)

## 🤝 参与开发

仔细阅读 [DragonOS 社区介绍文档]，可以帮助你了解社区的运作方式以及如何参与贡献。

欢迎查看 Issue 参与讨论与任务认领，或访问 DragonOS 论坛了解开发动态：[https://bbs.dragonos.org.cn](https://bbs.dragonos.org.cn)。

### 常用站点与联系方式

- 项目官网：**[DragonOS.org](https://dragonos.org)**
- 文档站：**[docs.dragonos.org](https://docs.dragonos.org)**
- 社区介绍文档：**[community.dragonos.org](https://community.dragonos.org)**
- 贡献者指南：[贡献者指南](https://community.dragonos.org/contributors/#%E7%A4%BE%E5%8C%BA)（亦可通过 [社区管理团队]、[SIGs] 与 [WGs] 联系对应负责人）
- QQ 群：476358494

## 🌟 赞助商致谢

特别感谢以下赞助商对 DragonOS 项目的慷慨支持（按时间倒序）：

- **[中国雅云](https://yacloud.net)** 雅安大数据产业园 - 🥇 长期支持者（自 2023 年）
- **[Tencent Cloud EdgeOne](https://edgeone.ai/?from=github)** 腾讯云 EdgeOne CDN

### CDN 赞助商

<div align="center">
  <p>本项目的 CDN 加速和安全防护由腾讯 EdgeOne 赞助。</p>
  <a href="https://edgeone.ai/?from=github">
    <img src="https://edgeone.ai/media/34fe3a45-492d-4ea4-ae5d-ea1087ca7b4b.png" alt="腾讯EdgeOne CDN" width="200">
  </a>
</div>

### 个人赞赏者列表

请见 [SUPPORTERS.md](./SUPPORTERS.md)

## 📜 开源协议

本项目采用 **GPLv2** 协议开放源代码，欢迎在遵守开源协议的前提下使用与贡献代码。

## 👩‍💻 Contributors

> *"开源因你而更加闪耀！"* ✨

**感谢所有提交代码、修复问题或审核 PR 的开发者**：

<a href="https://github.com/DragonOS-Community/dragonos/graphs/contributors">
  <img src="https://contrib.rocks/image?repo=dragonos-community/dragonos" />
</a>

[DragonOS 社区介绍文档]: https://community.dragonos.org/
[社区管理团队]: https://community.dragonos.org/governance/staff-info.html
[SIGs]: https://community.dragonos.org/sigs/
[WGs]: https://community.dragonos.org/wgs/
