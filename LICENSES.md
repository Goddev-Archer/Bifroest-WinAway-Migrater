# Third-Party Licenses & Open Source Transparency

Bifröst is built upon open source software, respecting digital freedom and developer transparency. This document details the licensing terms governing Bifröst itself, its bundled early-boot runtime environment, and all third-party libraries and utilities used within the migration pipeline.

---

## 1. Primary Project License

Bifröst is licensed under the **GNU General Public License v3.0 (GPLv3)** (or later).

> **Why GPLv3?**  
> Bifröst is an in-place migration engine dedicated to digital sovereignty and the free software ecosystem. GPLv3 guarantees that Bifröst and any future derivatives remain free, open source, and accessible to everyone, preventing proprietary lock-in and safeguarding user freedoms.

See [LICENSE](file:///c:/dev/Project%20Movement/LICENSE) for the full text of the GNU General Public License v3.0.

---

## 2. Linux Kernel & Early-Boot UKI Components

Bifröst packages a minimal, self-contained Unified Kernel Image (UKI, `bifrost-boot.efi`) to execute RAMFS-level migration operations prior to loading the main operating system. The binaries included in the UKI are governed by their respective upstream licenses:

| Component | Upstream Project | Upstream License | Description |
| :--- | :--- | :--- | :--- |
| **Linux Kernel** | [Linux Kernel Organization](https://kernel.org) | **GPL-2.0-only** (with Linux-syscall-note) | Minimal virtualized kernel (`linux-virt`) providing synthetic SCSI and VMBus storage drivers. |
| **BusyBox** | [BusyBox Project](https://busybox.net) | **GPL-2.0-only** | Minimal static Unix toolset (`sh`, `mount`, `umount`, `cp`, `find`). |
| **GNU Parted** | [Free Software Foundation](https://www.gnu.org/software/parted/) | **GPL-3.0-or-later** | GPT partition manipulation and boundary resizing (`parted`, `partprobe`). |
| **NTFS-3G** | [Tuxera / GitHub Community](https://github.com/tuxera/ntfs-3g) | **GPL-2.0-or-later** / **LGPL-2.0-or-later** | Safe NTFS mounting, cluster inspection (`ntfsresize`), and dirty-flag handling (`ntfsfix`). |
| **btrfs-progs** | [Btrfs Project](https://btrfs.readthedocs.io) | **GPL-2.0-only** | Filesystem creation (`mkfs.btrfs`) and subvolume management (`@root`, `@home`, `@blackbox`). |
| **systemd-boot** | [systemd Project](https://systemd.io) | **LGPL-2.1-or-later** | Minimal UEFI boot manager configuration deployed to the ESP. |

> **Note on GPL System Call Exception:**  
> The Linux kernel includes an explicit exception in its licensing (`LICENSES/exceptions/Linux-syscall-note`): standard user-space programs that interface with the kernel solely via system calls are **not** considered derived works of the Linux kernel.

---

## 3. Rust Dependencies & Crates

The Bifröst Windows binary (`bifroest.exe`) and Linux RAMFS binary (`bifroest`) are authored in Rust. They rely on the following open source crates:

| Crate | License | Purpose |
| :--- | :--- | :--- |
| **ratatui** | MIT | Terminal User Interface (TUI) layout, widgets, and cinema rendering engine. |
| **crossterm** | MIT | Cross-platform terminal control, alternate screen, and raw mode handling. |
| **zstd** | MIT / BSD-3-Clause | High-speed, high-ratio compression for the migration Blackbox archive. |
| **tar** | MIT / Apache-2.0 | POSIX archive packaging and streaming extraction. |
| **serde** / **serde_json** | MIT / Apache-2.0 | Migration manifest serialization and software candidate cataloging. |
| **walkdir** | Unlicense / MIT | Fast recursive filesystem traversal for Blackbox candidate discovery. |
| **sha2** | MIT / Apache-2.0 | Cryptographic SHA-256 verification of Blackbox archives. |
| **sysinfo** | MIT | Cross-platform hardware diagnostics, RAM, and CPU telemetry. |
| **winapi** | MIT / Apache-2.0 | Windows-specific Win32 API bindings for drive inspection and privilege queries. |

---

## 4. Source Code Availability & GPL Compliance

In compliance with the GNU General Public License v2.0 and v3.0:
- The full source code of Bifröst is publicly hosted at [GitHub](https://github.com/Goddev-Archer/Bifroest-WinAway-Migrater).
- The automated recipes and build scripts used to compile the Linux kernel and assemble the initramfs UKI are fully documented and reproducible via [tools/build_uki.sh](file:///c:/dev/Project%20Movement/tools/build_uki.sh), [tools/make_initramfs.sh](file:///c:/dev/Project%20Movement/tools/make_initramfs.sh), and [.github/workflows/build-uki.yml](file:///c:/dev/Project%20Movement/.github/workflows/build-uki.yml).
