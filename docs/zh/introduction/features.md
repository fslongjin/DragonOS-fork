# DragonOS 功能特性 {#_genreal_features}

DragonOS 是一套使用 Rust 从零构建的 64 位独立内核操作系统，面向 **Agent Native** 与轻量化云原生场景，提供 **Linux 二进制（ABI）兼容性**。系统默认采用 **Ubuntu 24.04** 作为 rootfs，可直接通过 `apt` 安装和管理软件包。以下概述 DragonOS 当前已经实现的核心子系统与生态能力。

## Agent 沙箱与容器隔离

围绕高密度 Agent 沙箱与容器化工作负载，DragonOS 在内核层提供了完整的隔离、资源管控与安全边界能力：

- **Cube Sandbox 兼容**：原生适配并兼容 **Cube Sandbox**，可直接作为 Cube Sandbox 的轻量级 Guest OS 运行各类 AI Agent 沙箱环境；在高密度部署场景下，相比 Linux 可实现约 **10%** 的内存节省。
- **Docker 兼容**：兼容 **Docker** 容器运行环境，支持标准容器工作负载的隔离与部署。
- **Linux Namespaces**：支持 `pid`、`mnt`（含挂载传播）、`net`、`ipc`、`uts`、`user`、`cgroup` 等主流命名空间，以及 `unshare` / `setns` 接口。
- **Cgroup v2**：支持统一层级资源控制，涵盖 `cpu`（含 CFS 带宽限流）、`cpuset`、`memory`、`pids`、`freezer` 以及 threaded 模式。
- **安全与执行控制**：支持 `seccomp` 系统调用过滤、POSIX 凭证与权限模型（`cred`）、内核密钥环（`keys`）、`ptrace` 调试跟踪以及 `rseq`（可重启序列）。

## 文件系统与存储

DragonOS 实现了统一的 VFS 架构，覆盖容器分层存储、宿主机共享目录以及现代事件通知机制：

- **核心机制**：统一 VFS、页缓存（Page Cache）与预读（Readahead）、`fsnotify` / `inotify`、`epoll` / `poll`、`eventfd`、`timerfd`、`signalfd`。
- **存储与沙箱文件系统**：
  - **OverlayFS**：支持联合挂载与写时复制，满足容器分层镜像需求。
  - **VirtIO-FS**：支持宿主机与 Guest OS 之间的高性能目录共享，契合 Agent 沙箱工作区挂载场景。
  - **FUSE**：支持用户态文件系统扩展。
  - **本地与内存文件系统**：支持 `Ext4`、`tmpfs`、`ramfs`、`FAT`。
- **伪文件系统**：完整提供 `procfs`、`sysfs`、`devfs`、`devpts`、`cgroup2`、`debugfs`、`kernfs`。

## 网络协议栈与虚拟网络

内置独立的现代化网络协议栈，兼顾云原生容器网络与 Guest-Host 通信需求：

- **核心协议栈**：支持 IPv4 / IPv6（含分片重组、源策略路由、邻居表、ICMP 差错处理），支持 TCP、UDP、Raw Socket、Unix Domain Socket、Packet Socket 及 Netlink（`rtnetlink`）。
- **Guest-Host 通信通道**：支持 **VirtIO-VSOCK**，无需复杂网络配置即可实现宿主机与 Agent 沙箱之间的低延迟通信。
- **容器与虚拟网络**：支持网络命名空间（NetNS）、`veth` 虚拟网卡对、软件网桥（`bridge`）、`nftables` 规则过滤与连接跟踪（`conntrack`）、NAPI 轮询收包机制。

## 可观测性与可编程内核

提供面向生产排障、安全审计与性能分析的内核级可观测能力：

- **eBPF 子系统**：支持 `sys_bpf` 系统调用、指令验证器（Verifier）、eBPF / cBPF 程序（含 cgroup device 等挂载点），支持 Array、Hash、LRU、Queue、RingBuf 等多种 Map 及常用 Helper 函数。
- **动态追踪与性能分析**：支持 `perf_event_open`、`kprobe`、`uprobe`、`tracepoint` 以及 `trace_pipe`。

## 进程管理、调度与 IPC

- **进程与线程管理**：支持完整的 POSIX 进程与线程模型（`fork` / `clone` / `execve` / `wait`）、`pidfd`、进程组与会话、shebang 脚本执行、POSIX 高精度定时器及完整信号机制。
- **多核调度器**：支持 CFS 公平调度（含组调度、CFS 带宽控制与 PELT 负载追踪）、实时调度（FIFO / RR）、多核 SMP 负载均衡与 CPU 亲和性设置。
- **进程间通信（IPC）**：支持匿名/命名管道（Pipe）、System V IPC（共享内存 `shm`、信号量 `sem` 及 `sem_undo`）、POSIX 消息队列（`mqueue`）以及 Futex / Robust Mutex。

## 内存管理与弹性机制

- **核心内存子系统**：支持页帧分配器、小对象分配器、VMA 管理、按需分页与写时复制（COW）、反向映射（RMAP）、LRU 页面回收与内存规整（Compaction）、`vmalloc` 以及 `madvise` / `mincore`。
- **稳定性与弹性回收**：内置 OOM Killer 与 OOM Reaper 机制；面向云计算与 Guest OS 场景，支持 `virtio-balloon`、页面报告（`page_reporting`）与 `virtio-pmem` 等内存弹性与持久化特性。

## 设备驱动、多架构与虚拟化支持

- **VirtIO 设备体系**：完整支持面向云原生 Guest OS 的 VirtIO 设备（`virtio-blk`、`virtio-net`、`virtio-fs`、`virtio-vsock`、`virtio-balloon`、`virtio-pmem`、`virtio-rng`），同时支持 PCI 与 MMIO 传输层。
- **基础硬件与块设备**：支持 `loop` 块设备、e1000e 网卡、AHCI、PCI / PCIe、ACPI、串口与 TTY / PTY、高精度时钟源（HPET / APIC / RTC）。
- **多架构支持**：以 **x86_64** 为主力架构，持续推进 **RISC-V 64** 与 **LoongArch64** 支持。
- **虚拟化扩展**：内核同时提供基础的 KVM 虚拟化支持。

## Linux 二进制兼容与软件生态

- **Linux ABI 兼容**：支持标准 ELF 静态/动态链接可执行文件，兼容 `glibc` 与 `musl` 用户态运行环境。
- **主流语言与服务端软件**：支持 C/C++、Go、Python、Rust 等主流编程语言及运行时环境；常见主流服务端软件（如 **MySQL**、**ClickHouse**、**Redis** 等）及常用工具（如 BusyBox、Dropbear）可在无需修改的情况下直接在 DragonOS 上运行。
- **系统调用与兼容性数据**：
  - 系统调用列表请参见：[系统调用文档](/zh/kernel/syscall/index.html)
  - 实时 Linux 兼容性测试数据请参见：[DragonOS CI Dashboard](https://ci-dashboard.dragonos.org)
