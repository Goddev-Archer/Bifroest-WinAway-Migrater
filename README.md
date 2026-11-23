```text
▄▄▄▄  ▄▄ ▄▄▄▄▄ ▄▄▄▄   ▄▄▄  ▄▄▄▄▄  ▄▄▄▄ ▄▄▄▄▄▄ 
██▄██ ██ ██▄▄  ██▄█▄ ██▀██ ██▄▄  ███▄▄   ██   
██▄█▀ ██ ██    ██ ██ ▀███▀ ██▄▄▄ ▄▄██▀   ██   
```

<div align="center">

**The Fully Automated, Zero-BIOS, Zero-USB In-Place Migration Engine from Windows 11 to Native Linux.**

[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](file:///c:/dev/Project%20Movement/LICENSES.md)
[![Rust](https://img.shields.io/badge/Rust-2021_Edition-orange.svg)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/Platform-Windows_11_%E2%86%92_Linux-cyan.svg)](#)
[![Zero BIOS](https://img.shields.io/badge/Zero--BIOS-100%25_Programmatic-brightgreen.svg)](#)

</div>

---

## 🌌 The Vision

Switching from Windows to Linux has historically been intimidating, manual, and prone to user error:
1. Burning live ISOs onto physical USB thumbdrives or DVDs.
2. Intercepting fast-boot motherboards to hammer `F2`, `F12`, or `Del` into BIOS/UEFI menus.
3. Manually calculating partition offsets, formatting swap/root, and losing personal files in the process.

**Bifröst completely eliminates this burden.**  
You run `bifroest.exe` from inside Windows. Bifröst packages your digital identity into an encrypted Blackbox, stages a Unified Kernel Image (UKI) into your EFI system partition, programs Windows Boot Manager for a one-time handoff, and restarts. You sit back and watch your machine cross the rainbow bridge into native Linux.

---

## ⚡ Core Architecture

```
┌────────────────────────────────────────────────────────────────────────┐
│ 1. WINDOWS RUNTIME (Live Execution & Packaging)                        │
├────────────────────────────────────────────────────────────────────────┤
│ • Scans user profile (SSH keys, GPG keys, Git repos, Docs, App configs)│
│ • Packages encrypted Blackbox: `bifrost_blackbox.tar.zst`              │
│ • Stages Distro Bootstrap Rootfs: `archlinux-bootstrap-x86_64.tar.zst` │
│ • Mounts ESP (`mountvol S: /S`) & deploys `bifrost-boot.efi`           │
│ • Arms BCD one-time boot sequence (`bcdedit /bootsequence {GUID}`)     │
│ • Triggers programmatic system reboot (No BIOS hotkeys required)       │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │ Reboot
┌───────────────────────────────────▼────────────────────────────────────┐
│ 2. BIFRÖST TRANSIT ENGINE (Linux RAMFS Early Boot)                     │
├────────────────────────────────────────────────────────────────────────┤
│ • Dispatches Unified Kernel Image directly from memory (`linux-virt`)  │
│ • Mounts Windows NTFS read-only and caches Blackbox + Rootfs into RAM  │
│ • Unmounts Windows cleanly (`umount /mnt/win_src`)                     │
│ • Executes Chosen Strategy:                                            │
│    - Mode A: Clean Install -> Wipes Windows, dedicates disk to Linux   │
│    - Mode B: Dual-Boot     -> Non-destructive `ntfsresize` + shrink    │
│ • Formats Btrfs subvolumes (`@root`, `@home`, `@blackbox`)             │
│ • Extracts Target Distro Rootfs into `@root`                           │
│ • Restores Blackbox from RAM into `@home` with strict POSIX modes      │
│ • Deploys systemd-boot to ESP & configures First-Boot Software Service │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │ Automated Clean Reboot
┌───────────────────────────────────▼────────────────────────────────────┐
│ 3. ASGARD ARRIVAL (Native Linux)                                       │
├────────────────────────────────────────────────────────────────────────┤
│ • Boots straight into your new native Linux desktop / shell            │
│ • 100% of your personal files, SSH credentials, and settings restored  │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 🚀 Key Features

- **Zero-BIOS, Zero-USB, Zero-DVD**: Handled completely in software through programmatic UEFI BCD orchestration.
- **In-Memory Transit (RAM-First Safety)**: Before any partition or filesystem operation begins, your Blackbox archive and installation images are buffered directly into RAM (`/tmp/`). Even if Windows is completely wiped, your personal data is protected and ready for injection.
- **Migration Strategy Freedom**:
  - **Clean Install (Recommended)**: Wipes Windows entirely and dedicates 100% of your disk to Linux Btrfs subvolumes. Zero NTFS resizing risks.
  - **Dual-Boot**: Automatically shrinks NTFS using `ntfsresize` with deep cluster inspection to safely make room for Linux alongside Windows.
- **Btrfs Subvolume Topology**: Native snapshot support right out of the box (`@root`, `@home`, `@blackbox`) with zstd compression enabled.
- **Identity & Credential Preservation**: Automatically maps and enforces POSIX permissions (`0700` for `.ssh`, `0600` for private keys, `0644` for public keys and configs).
- **The Bifröst Continuum Cinema TUI**: Real-time early-boot telemetry wrapped in an animated terminal scene complete with Norse rune streams and particle accelerators.

---

## 🛠️ The Bifröst Cinema Interface

During early-boot transit, Bifröst takes over the display to provide real-time visual progress without letting raw kernel logs bleed through:

```
 ______     ______     __     _____     ______     ______    
/\  == \   /\  == \   /\ \   /\  __-.  /\  ___\   /\  ___\     THE BRIDGE TO ASGARD
\ \  __<   \ \  __<   \ \ \  \ \ \/\ \ \ \ \__ \  \ \  __\     Elapsed: 01:42
 \ \_____\  \ \_\ \_\  \ \_\  \ \____-  \ \_____\  \ \_____\ 
  \/_____/   \/_/ /_/   \/_/   \/____/   \/_____/   \/_____/ 

╔══════════════════════════ ⚡ BIFRÖST CONTINUUM ⚡ ═════════════════════════╗
║ [ MIDGARD / WIN11 ] ╒══─══─══─══─══─══─══─══─══╕ [ ASGARD / LINUX ]         ║
║   NTFS Volume       │»»»»»» [■ BLACKBOX] »»»»»»│   Btrfs Subvols            ║
║     (Source)        ╘══─══─══─══─══─══─══─══─══╛     (Target)               ║
╚═════════════════════════════════════════════════════════════════════════════╝
```

---

## 📦 Project Structure

```
├── .github/workflows/       # Automated UKI & cross-compilation CI pipelines
├── src/
│   ├── blackbox/            # Identity extraction, scanning, and tar.zst packaging
│   ├── bridge/              # BCD handoff, partitioning, Btrfs layout & Cinema TUI
│   │   ├── bootstrap.rs     # systemd-boot deployment & first-boot scripts
│   │   ├── handoff.rs       # Automated ESP mounting and Windows BCD registration
│   │   ├── partition.rs     # GPT / parted / ntfsresize execution engine
│   │   └── ramfs_cinema.rs  # Ratatui-based Bifröst Bridge live animation engine
│   ├── main.rs              # Windows wizard entrypoint and RAMFS PID-1 executor
│   └── model.rs             # Candidate data structures, manifests & telemetry
├── tools/                   # UKI builder scripts (Alpine linux-virt, Busybox, CPIO)
├── AGENTS.md                # Directives and architectural requirements for Argon
├── LICENSES.md              # Open source transparency and third-party licenses
└── Cargo.toml               # Rust workspace definition
```

---

## 📜 Licensing & Open Source Transparency

Bifröst is licensed under the **GNU General Public License v3.0 (GPLv3)**.

All bundled components within the early-boot UKI (such as the Linux kernel, Busybox, NTFS-3G, and GNU Parted) are free software governed by their respective upstream licenses.  
For an exhaustive breakdown of third-party licenses, the Linux system-call exception, and dependency crate notices, please see [LICENSES.md](file:///c:/dev/Project%20Movement/LICENSES.md).
