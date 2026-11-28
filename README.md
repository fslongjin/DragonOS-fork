<div align="center">
  <img width="40%" src="docs/_static/dragonos-logo.svg" alt="dragonos-logo"><br/>
  <h2>Lightweight Rust OS Kernel for the Agent Native Era</h2>

<a href="https://dragonos.org"><img alt="Website" src="https://img.shields.io/badge/Website-DragonOS.org-4c69e4"></a>
<a href="https://docs.dragonos.org"><img alt="Docs" src="https://img.shields.io/badge/Docs-docs.dragonos.org-2ea44f"></a>
<a href="https://ci-dashboard.dragonos.org"><img alt="CI Dashboard" src="https://img.shields.io/badge/CI_Dashboard-Linux_Compatibility-orange"></a>
<a href="https://bbs.dragonos.org.cn"><img alt="BBS" src="https://img.shields.io/badge/BBS-bbs.dragonos.org.cn-purple"></a>
<a href="./LICENSE"><img alt="License: GPLv2" src="https://img.shields.io/badge/License-GPLv2-blue"></a>

---

</div>

# DragonOS

**Languages**: [中文](README_CN.md) | English

DragonOS is a lightweight 64-bit operating system with an independent kernel built from scratch in Rust for the **Agent Native** era, providing **Linux binary (ABI) compatibility**. Purpose-built as a streamlined **Guest OS**, it sheds the legacy baggage of traditional general-purpose kernels to run **AI Agent sandboxes** and **cloud-native services** efficiently and securely.

🌟 **Want to quickly experience DragonOS?** Visit [DragonOS Playground](https://cnb.cool/DragonOS-Community/playground) to launch DragonOS in the cloud with zero configuration and try the latest nightly build!

## ✨ Highlights

- **Built for Agent Sandboxes & Containers**: Natively compatible with **Cube Sandbox** (achieving approximately **10%** memory savings compared to Linux in high-density deployments) and **Docker**, with full kernel support for Linux Namespaces, `Cgroup v2`, and `seccomp`.
- **High Linux Compatibility & Ready-to-Use Ecosystem**: Uses **Ubuntu 24.04** as the default RootFS on `x86_64` with direct `apt` package management; supports both dynamic and static linking (`glibc` / `musl`) and mainstream languages (C/C++, Go, Python, Rust); widely used server software such as **MySQL, ClickHouse, and Redis** run directly on DragonOS without modification.
- **Independent Rust Kernel & Modern Subsystems**:
  - **Storage & Filesystems**: Unified VFS, Page Cache, OverlayFS, VirtIO-FS, FUSE, Ext4, `tmpfs`, `procfs` / `sysfs` / `cgroup2`.
  - **Networking & Guest-Host Communication**: Independent IPv4 / IPv6 stack, NetNS, `veth`, software bridges, `nftables` / `conntrack`, and **VirtIO-VSOCK**.
  - **Observability & Elasticity**: eBPF, `kprobe`, `uprobe`, `tracepoint`, `perf_event_open`, along with `virtio-balloon` and OOM Killer / Reaper for cloud-native Guest environments.

Explore the full list of capabilities in [Features](https://docs.dragonos.org/introduction/features.html#_genreal_features), or check live Linux compatibility metrics on the [DragonOS CI Dashboard](https://ci-dashboard.dragonos.org).

## 📰 Community News

- **2025-12-22**: 🚀 **DragonOS 0.4.0 has been released!** Focused on Linux semantic alignment and container/cloud-native foundations, with major enhancements across I/O multiplexing, POSIX timers, `tmpfs`, mount propagation, and `procfs`. [Read the changelog →](https://docs.dragonos.org/community/ChangeLog/V0.4.x/V0.4.0.html)
- **2025-12-17**: ☁️ **DragonOS Playground is now live!** Experience DragonOS with zero configuration on the CNB platform with daily nightly builds! [Try it now →](https://cnb.cool/DragonOS-Community/playground)
- **2025-11-20**: 🚀 DragonOS 0.3.0 has been released! Supports running Go programs directly!
- **2025-11-18**: 📊 DragonOS CI Dashboard is now live! [Check out DragonOS Linux compatibility data →](https://ci-dashboard.dragonos.org)

## 🚀 How to Run

### 🌟 Method 1: Cloud-Native Experience (Playground)

Zero configuration, one-click launch on the CNB cloud-native platform—no local dependencies required:

<div align="center">

[![Launch on CNB](docs/_static/cnb-launch-button-en.svg)](https://cnb.cool/DragonOS-Community/playground)

</div>

### Method 2: Local Build & Deployment

For local development, we recommend initializing your environment with `tools/bootstrap.sh`. You can also package your own applications and files by building a local Docker image and referencing it in a RootFS Manifest:

- [Building DragonOS — DragonOS Documentation](https://docs.dragonos.org/introduction/build_system.html)
- [Application Development & Deployment Guide (Docker RootFS Customization)](https://docs.dragonos.org/userland/appdev/)

## 🤝 Want to Contribute?

Read the [DragonOS Community Introduction Document] to learn how the community operates and how you can contribute.

Feel free to browse issues, join discussions, or visit the DragonOS forum to stay updated on development progress: [https://bbs.dragonos.org.cn](https://bbs.dragonos.org.cn).

### Sites & Community

- Official Website: **[DragonOS.org](https://dragonos.org)**
- Documentation: **[docs.dragonos.org](https://docs.dragonos.org)**
- Community Portal: **[community.dragonos.org](https://community.dragonos.org)**
- Contributor Guide: [Contributor Guide](https://community.dragonos.org/contributors/#%E7%A4%BE%E5%8C%BA) (see also [Community Management Team], [SIGs], and [WGs])
- QQ Group: 476358494

## 🌟 Sponsors

Special thanks to these generous financial supporters (in reverse chronological order):

- **[中国雅云](https://yacloud.net)** 雅安大数据产业园 - 🥇 Long-term supporter (since 2023)
- **[Tencent Cloud EdgeOne](https://edgeone.ai/?from=github)** EdgeOne CDN by TencentCloud

### CDN Sponsor

<div align="center">
  <p>This project's CDN acceleration and security protection are sponsored by Tencent EdgeOne.</p>
  <a href="https://edgeone.ai/?from=github">
    <img src="https://edgeone.ai/media/34fe3a45-492d-4ea4-ae5d-ea1087ca7b4b.png" alt="Tencent EdgeOne CDN" width="200">
  </a>
</div>

### Individual Sponsors

See [SUPPORTERS.md](./SUPPORTERS.md)

## 📜 License

This project is open-sourced under the **GPLv2** license. You are welcome to use and contribute to the code in compliance with the license.

## 👩‍💻 Contributors

> *"Open source shines because of you!"* ✨

**Thanks to all developers who submitted code, fixed issues, or reviewed PRs**:

<a href="https://github.com/DragonOS-Community/dragonos/graphs/contributors">
  <img src="https://contrib.rocks/image?repo=dragonos-community/dragonos" />
</a>

[DragonOS Community Introduction Document]: https://community.dragonos.org/
[Community Management Team]: https://community.dragonos.org/governance/staff-info.html
[SIGs]: https://community.dragonos.org/sigs/
[WGs]: https://community.dragonos.org/wgs/
