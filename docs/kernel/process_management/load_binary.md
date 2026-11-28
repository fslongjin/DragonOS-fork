# Binary Loader

## 1. Binary & Script Loading Flow

This section summarizes how `execve` loads binaries and scripts in DragonOS (implemented in `kernel/src/process/exec.rs`, `kernel/src/process/shebang.rs`, and `kernel/src/libs/elf.rs`).

DragonOS follows a **"probe-and-load"** pipeline when executing a program:

1. **Shebang Probing**: First checks whether the file starts with `#!`. For scripts, the shebang loader parses the interpreter path and optional argument, then re-invokes loading for the target interpreter (bounded by `SHEBANG_MAX_RECURSION_DEPTH`).
2. **Binary Format Probing**: Reads the file header and invokes `probe` on registered `BinaryLoader` implementations to identify a matching loader.
3. **Loading & Mapping**: Maps program segments into the new user address space, sets up the user stack, arguments (`argv`), environment variables (`envp`), and auxiliary vector (`auxv`), and returns the entry point address.

## 2. Supported Formats & Linking Modes

- **ELF Loader (`ELF_LOADER`)**: Supports standard 64-bit ELF executables, including both **statically linked** and **dynamically linked** binaries (handling `PT_INTERP` to load dynamic linkers such as `glibc` / `musl` `ld.so` and populating `AT_PHDR`, `AT_ENTRY`, `AT_BASE`, etc. in `auxv`), as well as PIE executables.
- **Shebang Loader (`SHEBANG_LOADER`)**: Supports script executables such as `#!/bin/sh` and `#!/usr/bin/env python3`.
