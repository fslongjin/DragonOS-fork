# Introduction to DragonOS

DragonOS is a lightweight 64-bit operating system with an independent kernel built from scratch in Rust for the **Agent Native** era, providing **Linux binary (ABI) compatibility**. It is designed to run **AI Agent sandboxes** and **cloud-native services** efficiently and securely.

## Why Agent Native?

As AI Agents increasingly execute code, invoke external tools, and orchestrate complex workflows, the underlying operating system faces new infrastructure requirements:

- **High-Density, Lightweight Agent Sandboxes**: Agent runtimes frequently spin up and tear down isolated environments, demanding fast boot times, low memory overhead, and a minimal attack surface. As a lightweight **Guest OS**, DragonOS sheds the legacy baggage of traditional general-purpose kernels. Leveraging Rust's memory safety and a streamlined kernel architecture—alongside mechanisms such as Namespaces and Cgroups—it provides efficient, reliable process isolation and resource control inside sandboxes, making it well-suited for large-scale, high-density Agent execution.
- **Rich Language Runtimes and Mainstream Service Ecosystem**: Agent code execution, tool services, data storage, and long-running backend services rely heavily on the mature Linux ecosystem. Committed to Linux binary compatibility while continuously advancing container support, networking, scheduling, and eBPF, DragonOS supports mainstream programming languages and runtimes (such as C/C++, Go, Python, and Rust), and enables widely used server software like MySQL and ClickHouse to run directly on DragonOS without modification.

## Technical Highlights & Evolution

- **Independent Rust Kernel**: Core subsystems are written in Rust, significantly reducing memory-safety vulnerabilities at the language level and improving system stability under multi-tenant, high-concurrency workloads.
- **Streamlined Cloud-Native Guest Architecture**: Purpose-built for lightweight cloud computing and Guest OS environments, keeping device models (such as VirtIO), scheduling, networking, and isolation mechanisms clean and efficient, while actively advancing cloud platform integration and multi-architecture support (such as RISC-V).
- **Continuously Evolving Linux Compatibility**: DragonOS is steadily expanding its syscall and kernel interface coverage to improve compatibility with production-grade Linux applications and containerized workloads. You can track the latest compatibility test results on the [DragonOS CI Dashboard](https://ci-dashboard.dragonos.org).

## Open Source & License

Launched as an open-source project in July 2022, DragonOS is released under the **GPLv2** license. We welcome developers and teams interested in operating system kernels, Agent infrastructure, and cloud-native technologies to join us in building the ecosystem.

If you would like to explore the features already implemented in DragonOS, please visit [Features](/introduction/features.html#_genreal_features).
