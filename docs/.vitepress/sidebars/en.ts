export const enSidebar = [
    {
      text: 'Getting Started',
      collapsed: false,
      items: [
      { text: 'Introduction to DragonOS', link: '/introduction/' },
      { text: 'Features', link: '/introduction/features' },
      { text: 'Building DragonOS', link: '/introduction/build_system' },
      { text: 'Developing DragonOS with Nix', link: '/introduction/develop_nix' },
      { text: 'Developing DragonOS with devcontainer', link: '/introduction/devcontainer' },
      { text: 'DragonOS Mirror Site', link: '/introduction/mirrors' }
      ]
    },
    {
      text: 'Kernel Layer',
      collapsed: false,
      items: [
      {
        text: 'Kernel Compilation Configuration',
        collapsed: true,
        items: [
        { text: 'Kernel Compilation Configuration Guide', link: '/kernel/configuration/config' },
        { text: 'Target Architecture Configuration', link: '/kernel/configuration/arch' }
        ]
      },
      {
        text: 'Bootloader',
        collapsed: true,
        items: [
        { text: 'Bootloader', link: '/kernel/boot/bootloader' },
        { text: 'Kernel Boot Command Line Parameters', link: '/kernel/boot/cmdline' }
        ]
      },
      {
        text: 'Core API Documentation',
        collapsed: true,
        items: [
        { text: 'DragonOS Kernel Core API', link: '/kernel/core_api/kernel_api' },
        { text: 'Atomic Variables', link: '/kernel/core_api/atomic' },
        { text: 'Type Conversion Library API', link: '/kernel/core_api/casting' },
        { text: 'Notifier Chain', link: '/kernel/core_api/notifier_chain' },
        { text: 'Soft Interrupt', link: '/kernel/core_api/softirq' }
        ]
      },
      {
        text: 'Interrupts and Bottom Halves',
        collapsed: true,
        items: [
        { text: 'tasklet', link: '/kernel/interrupt/tasklet' },
        { text: 'DragonOS Workqueue Mechanism Design Document', link: '/kernel/interrupt/workqueue' }
        ]
      },
      {
        text: 'Locks',
        collapsed: true,
        items: [
        { text: 'Types of Locks and Their Rules', link: '/kernel/locking/locks' },
        { text: 'Spinlock', link: '/kernel/locking/spinlock' },
        { text: 'RwLock Read-Write Lock', link: '/kernel/locking/rwlock' },
        { text: 'mutex (Mutual Exclusion Lock)', link: '/kernel/locking/mutex' },
        { text: 'RwSem Read-Write Semaphore', link: '/kernel/locking/rwsem' }
        ]
      },
      {
        text: 'RCU',
        collapsed: true,
        items: [
        { text: 'DragonOS RCU Architecture', link: '/kernel/rcu/architecture' },
        { text: 'RCU Segmented Callback Queues', link: '/kernel/rcu/segmented-callback-queues' },
        { text: 'DragonOS SRCU Design Principles', link: '/kernel/rcu/srcu-design' }
        ]
      },
      {
        text: 'Process Management Module',
        collapsed: true,
        items: [
        { text: 'kthread Kernel Threads', link: '/kernel/process_management/kthread' },
        { text: 'Binary Loader', link: '/kernel/process_management/load_binary' },
        { text: 'Principle of DragonOS Multi-threaded Exec (De-thread) Mechanism', link: '/kernel/process_management/de_thread' }
        ]
      },
      {
        text: 'DragonOS Scheduling',
        collapsed: true,
        items: [
        { text: 'APIs Related to Process Scheduler', link: '/kernel/sched/core' },
        { text: 'APIs Related to the Completely Fair Scheduler', link: '/kernel/sched/cfs' },
        { text: 'FIFO Scheduler', link: '/kernel/sched/fifo' },
        { text: 'APIs Related to Real-Time Process Scheduler', link: '/kernel/sched/rt' },
        { text: 'Kernel Timer', link: '/kernel/sched/kernel_timer' },
        { text: 'DragonOS Wait Queue Mechanism', link: '/kernel/sched/wait_queue' }
        ]
      },
      {
        text: 'Interprocess Communication',
        collapsed: true,
        items: [
        { text: 'Signal', link: '/kernel/ipc/signal' },
        { text: 'IPC Namespace', link: '/kernel/ipc/ipc_namespace' },
        { text: 'Restartable Sequences (rseq) Mechanism', link: '/kernel/ipc/rseq' }
        ]
      },
      {
        text: 'Memory Management',
        collapsed: true,
        items: [
        { text: 'Introduction to the Memory Management Module', link: '/kernel/memory_management/intro' },
        { text: 'Memory Allocation Guide', link: '/kernel/memory_management/allocate-memory' },
        { text: 'MMIO', link: '/kernel/memory_management/mmio' },
        { text: 'Secure Memory Copy Scheme Based on Exception Table', link: '/kernel/memory_management/extable_safe_copy_design' },
        { text: 'OOM Killer Design', link: '/kernel/memory_management/oom_killer' }
        ]
      },
      {
        text: 'File System',
        collapsed: true,
        items: [
        { text: 'Overview', link: '/kernel/filesystem/overview' },
        { text: 'DragonOS inotify: User Semantics and Architecture', link: '/kernel/filesystem/inotify' },
        { text: 'DragonOS FUSE Architecture Design', link: '/kernel/filesystem/fuse' },
        { text: 'Virtiofs Benchmark Runbook', link: '/kernel/filesystem/virtiofs_benchmark_runbook' },
        {
          text: 'VFS Virtual File System',
          collapsed: true,
          items: [
          { text: 'Design', link: '/kernel/filesystem/vfs/design' },
          { text: 'Mount Propagation Mechanism', link: '/kernel/filesystem/vfs/mount_propagation' },
          { text: 'Mountable Filesystem', link: '/kernel/filesystem/vfs/mountable_fs' }
          ]
        },
        {
          text: 'ProcFS',
          collapsed: true,
          items: [
          { text: 'Proc Mount Export Interface', link: '/kernel/filesystem/proc/mounts' }
          ]
        },
        { text: 'SysFS', link: '/kernel/filesystem/sysfs' },
        { text: 'KernFS', link: '/kernel/filesystem/kernfs' },
        {
          text: 'Union Filesystem',
          collapsed: true,
          items: [
          { text: 'overlayfs', link: '/kernel/filesystem/unionfs/overlayfs' }
          ]
        }
        ]
      },
      {
        text: 'Kernel Debug Module',
        collapsed: true,
        items: [
        { text: 'Kernel Stack Traceback', link: '/kernel/debug/traceback' },
        { text: 'How to Use GDB to Debug the Kernel', link: '/kernel/debug/debug-kernel-with-gdb' },
        { text: 'Performance Analysis of the Kernel Using DADK', link: '/kernel/debug/profiling-kernel-with-dadk' }
        ]
      },
      {
        text: 'Kernel Testing',
        collapsed: true,
        items: [
        { text: 'dunitest Userspace Test Framework', link: '/kernel/ktest/dunitest' },
        { text: 'gVisor System Call Testing', link: '/kernel/ktest/gvisor_syscall_test' }
        ]
      },
      {
        text: 'Processor Architecture',
        collapsed: true,
        items: [
        {
          text: 'x86-64 Related Documentation',
          collapsed: true,
          items: [
          { text: 'USB Legacy Support', link: '/kernel/cpu_arch/x86_64/usb_legacy_support' }
          ]
        }
        ]
      },
      {
        text: 'Other Kernel Libraries',
        collapsed: true,
        items: [
        { text: 'unified-init Unified Initialization Library', link: '/kernel/libs/unified-init' },
        { text: 'ID Allocation', link: '/kernel/libs/id-allocation' }
        ]
      },
      {
        text: 'Network Subsystem',
        collapsed: true,
        items: [
        { text: 'Internet Protocol Socket', link: '/kernel/net/inet' },
        { text: 'UNIX', link: '/kernel/net/unix' },
        { text: 'SSH Support', link: '/kernel/net/ssh' },
        { text: 'Design Documentation of NAPI and NetNamespace Polling Mechanism in DragonOS', link: '/kernel/net/napi_and_netns_poll' }
        ]
      },
      {
        text: 'Kernel Tracing Mechanism',
        collapsed: true,
        items: [
        { text: 'Tracepoints', link: '/kernel/trace/tracepoint' },
        { text: 'DragonOS Runtime Text Patching: Principles, Usage, and Safety Boundaries', link: '/kernel/trace/text_patching' },
        { text: 'kprobe', link: '/kernel/trace/kprobe' },
        { text: 'Uprobe: Dynamic Probes for User Space', link: '/kernel/trace/uprobe' },
        { text: 'eBPF', link: '/kernel/trace/eBPF' }
        ]
      },
      {
        text: 'System Calls',
        collapsed: true,
        items: [
        { text: 'System Call Table Implementation Plan', link: '/kernel/syscall/syscall_table' },
        { text: 'Design Documentation for sys_capget / sys_capset', link: '/kernel/syscall/sys_capget_capset' }
        ]
      },
      {
        text: 'Device',
        collapsed: true,
        items: [
        { text: 'Linux tty Devices', link: '/kernel/device/tty' },
        { text: 'Loop Device Architecture Design', link: '/kernel/device/loop_device' }
        ]
      }
      ]
    },
    {
      text: 'Application Layer',
      collapsed: false,
      items: [
      {
        text: 'Application Development & Deployment',
        link: '/userland/appdev/'
      },
      {
        text: 'RootFS Build & Configuration',
        collapsed: true,
        link: '/userland/rootfs/',
        items: [
        { text: 'RootFS Manifest Configuration (Docker Image / DADK)', link: '/userland/rootfs/dadk-rootfs-manifest' },
        { text: 'From Packages to RootFS Image (Nix)', link: '/userland/rootfs/diskgen' },
        { text: 'Adding Built-in Programs in Nix', link: '/userland/rootfs/add-your-own-app' }
        ]
      }
      ]
    },
    {
      text: 'Development Guide',
      collapsed: false,
      items: [
      { text: 'Nix Tips', link: '/dev-skills/nix-skills' },
      { text: 'Contributing Documentation', link: '/dev-skills/write-docs' }
      ]
    },
    {
      text: 'Q&A',
      collapsed: false,
      items: [
      {
        text: 'FAQ',
        collapsed: true,
        items: [
        { text: 'Common Build Issues', link: '/questions/build_errors' }
        ]
      }
      ]
    },
    {
      text: 'DragonOS Community',
      collapsed: false,
      items: [
      {
        text: 'Contribute',
        collapsed: true,
        items: [
        { text: 'How to Contribute', link: 'https://community.dragonos.org/contributors/' },
        { text: 'Rust Coding Style', link: '/community/code_contribution/rust-coding-style' },
        { text: 'C Coding Style', link: '/community/code_contribution/c-coding-style' },
        { text: 'Conventional Commit Guidelines', link: '/community/code_contribution/conventional-commit' }
        ]
      },
      { text: 'Contact the Community', link: '/community/contact/' },
      {
        text: 'Changelog',
        collapsed: true,
        items: [
        { text: 'V0.4.0', link: '/community/ChangeLog/V0.4.x/V0.4.0' },
        { text: 'V0.3.0', link: '/community/ChangeLog/V0.3.x/V0.3.0' },
        { text: 'V0.2.0', link: '/community/ChangeLog/V0.2.x/V0.2.0' },
        { text: 'V0.1.10', link: '/community/ChangeLog/V0.1.x/V0.1.10' },
        { text: 'V0.1.9', link: '/community/ChangeLog/V0.1.x/V0.1.9' },
        { text: 'V0.1.8', link: '/community/ChangeLog/V0.1.x/V0.1.8' },
        { text: 'V0.1.7', link: '/community/ChangeLog/V0.1.x/V0.1.7' },
        { text: 'V0.1.6', link: '/community/ChangeLog/V0.1.x/V0.1.6' },
        { text: 'V0.1.5', link: '/community/ChangeLog/V0.1.x/V0.1.5' },
        { text: 'V0.1.4', link: '/community/ChangeLog/V0.1.x/V0.1.4' },
        { text: 'V0.1.3', link: '/community/ChangeLog/V0.1.x/V0.1.3' },
        { text: 'V0.1.2', link: '/community/ChangeLog/V0.1.x/V0.1.2' },
        { text: 'V0.1.1', link: '/community/ChangeLog/V0.1.x/V0.1.1' },
        { text: 'V0.1.0', link: '/community/ChangeLog/V0.1.x/V0.1.0' }
        ]
      }
      ]
    }
]
