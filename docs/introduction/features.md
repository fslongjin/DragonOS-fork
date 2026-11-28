# Features of DragonOS {#_genreal_features}

DragonOS is a 64-bit operating system with an independent kernel built from scratch in Rust, designed for **Agent Native** and lightweight cloud-native scenarios with **Linux binary (ABI) compatibility**. By default, it uses **Ubuntu 24.04** as its rootfs, allowing users to install and manage packages directly via `apt`. Below is an overview of the core subsystems and ecosystem capabilities currently implemented in DragonOS.

## Agent Sandbox & Container Isolation

To support high-density Agent sandboxes and containerized workloads, DragonOS provides comprehensive kernel-level isolation, resource control, and security boundaries:

- **Cube Sandbox Compatibility**: Natively adapted for and compatible with **Cube Sandbox**, running directly as a lightweight Guest OS for AI Agent sandboxes; in high-density deployment scenarios, it achieves approximately **10%** memory savings compared to Linux.
- **Docker Compatibility**: Compatible with the **Docker** container runtime environment, supporting the isolation and deployment of standard containerized workloads.
- **Linux Namespaces**: Supports mainstream namespaces including `pid`, `mnt` (with mount propagation), `net`, `ipc`, `uts`, `user`, and `cgroup`, along with `unshare` and `setns`.
- **Cgroup v2**: Supports unified hierarchy resource control, covering `cpu` (including CFS bandwidth throttling), `cpuset`, `memory`, `pids`, `freezer`, and threaded mode.
- **Security & Execution Control**: Supports `seccomp` syscall filtering, POSIX credentials and permissions (`cred`), kernel keyrings (`keys`), `ptrace` debugging/tracing, and `rseq` (restartable sequences).

## Filesystems & Storage

DragonOS implements a unified VFS architecture covering container layered storage, host-guest directory sharing, and modern event notification mechanisms:

- **Core Mechanisms**: Unified VFS, Page Cache and Readahead, `fsnotify` / `inotify`, `epoll` / `poll`, `eventfd`, `timerfd`, and `signalfd`.
- **Storage & Sandbox Filesystems**:
  - **OverlayFS**: Supports union mounting and copy-on-write for layered container images.
  - **VirtIO-FS**: Provides high-performance directory sharing between the host and Guest OS, ideal for mounting Agent sandbox workspaces.
  - **FUSE**: Supports userspace filesystem extensions.
  - **Local & In-Memory Filesystems**: Supports `Ext4`, `tmpfs`, `ramfs`, and `FAT`.
- **Pseudo Filesystems**: Provides `procfs`, `sysfs`, `devfs`, `devpts`, `cgroup2`, `debugfs`, and `kernfs`.

## Networking Stack & Virtual Networking

Built with an independent, modern networking stack tailored for both cloud-native container networking and Guest-Host communication:

- **Core Protocol Stack**: Supports IPv4 / IPv6 (including fragmentation/reassembly, source policy routing, neighbor tables, and ICMP error handling), TCP, UDP, Raw Sockets, Unix Domain Sockets, Packet Sockets, and Netlink (`rtnetlink`).
- **Guest-Host Channel**: Supports **VirtIO-VSOCK**, enabling low-latency communication between the host and Agent sandboxes without complex network configuration.
- **Container & Virtual Networking**: Supports Network Namespaces (NetNS), `veth` pairs, software bridges (`bridge`), `nftables` filtering with connection tracking (`conntrack`), and NAPI polling.

## Observability & Programmable Kernel

Provides kernel-level observability for production troubleshooting, security auditing, and performance analysis:

- **eBPF Subsystem**: Supports the `sys_bpf` syscall, bytecode verifier, eBPF / cBPF programs (including cgroup device hooks), multiple Map types (Array, Hash, LRU, Queue, RingBuf), and common helper functions.
- **Dynamic Tracing & Profiling**: Supports `perf_event_open`, `kprobe`, `uprobe`, `tracepoint`, and `trace_pipe`.

## Process Management, Scheduling & IPC

- **Processes & Threads**: Supports the full POSIX process and thread model (`fork` / `clone` / `execve` / `wait`), `pidfd`, process groups and sessions, shebang script execution, POSIX high-resolution timers, and full signal handling.
- **Multicore Scheduler**: Supports CFS fair scheduling (including group scheduling, CFS bandwidth control, and PELT load tracking), real-time scheduling (FIFO / RR), SMP multicore load balancing, and CPU affinity.
- **Inter-Process Communication (IPC)**: Supports anonymous/named pipes, System V IPC (shared memory `shm`, semaphores `sem` with `sem_undo`), POSIX message queues (`mqueue`), and Futex / Robust Mutex.

## Memory Management & Elasticity

- **Core Memory Subsystem**: Supports page frame allocation, slab/small-object allocation, VMA management, demand paging and copy-on-write (COW), reverse mapping (RMAP), LRU page reclamation, memory compaction, `vmalloc`, and `madvise` / `mincore`.
- **Stability & Elastic Reclamation**: Built-in OOM Killer and OOM Reaper; tailored for cloud and Guest OS environments with `virtio-balloon`, `page_reporting`, and `virtio-pmem` support.

## Device Drivers, Multi-Architecture & Virtualization

- **VirtIO Device Suite**: Full support for cloud-native Guest OS VirtIO devices (`virtio-blk`, `virtio-net`, `virtio-fs`, `virtio-vsock`, `virtio-balloon`, `virtio-pmem`, `virtio-rng`), over both PCI and MMIO transports.
- **Hardware & Block Devices**: Supports `loop` block devices, e1000e NICs, AHCI, PCI / PCIe, ACPI, serial/TTY/PTY, and high-resolution clocksources (HPET / APIC / RTC).
- **Multi-Architecture Support**: Primarily targets **x86_64**, while actively advancing **RISC-V 64** and **LoongArch64** support.
- **Virtualization Extension**: Also provides basic in-kernel KVM virtualization support.

## Linux Binary Compatibility & Software Ecosystem

- **Linux ABI Compatibility**: Runs standard static and dynamic ELF executables, compatible with both `glibc` and `musl` userspace environments.
- **Languages & Mainstream Server Software**: Supports major programming languages and runtimes (C/C++, Go, Python, Rust, etc.); widely used server software (such as **MySQL**, **ClickHouse**, and **Redis**) as well as common utilities (such as BusyBox and Dropbear) run directly on DragonOS without modification.
- **Syscalls & Compatibility Data**:
  - For the syscall list, see [Syscall Documentation](/kernel/syscall/index.html)
  - For live Linux compatibility test results, visit the [DragonOS CI Dashboard](https://ci-dashboard.dragonos.org)
