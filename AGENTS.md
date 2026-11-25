# AGENTS.md – Directives for Argon: Zero-BIOS-Interaction Migration Pipeline

> **Role & Assignment:** Argon (Orchestration & Systems Integration Agent)  
> **Mission:** Transform Bifröst from an interactive prototype into a **fully automated, production-grade, zero-BIOS, zero-USB, zero-DVD in-place migration engine** from Windows 11 to native Linux (Arch Linux, Fedora, Debian).  
> **Core Architectural Principle:** The user must NEVER be forced to enter BIOS/UEFI settings, burn an ISO, or insert physical boot media. The entire migration must happen programmatically from within Windows, transitioning seamlessly into Linux RAMFS.

---

## 1. Architectural Status & Lessons Learned (v0.2.0)

### What Succeeded:
1. **Zero-BIOS Hyper-V & Physical Handoff:** UEFI booting directly into `bifrost-boot.efi` via ESP staging (`S:\EFI\Bifrost\bifrost-boot.efi`) and Windows BCD one-time boot arming (`bcdedit /bootsequence`).
2. **Hyper-V Storage Discovery:** Alpine `linux-virt` kernel with built-in `CONFIG_SCSI_HYPERV=y` and `CONFIG_HYPERV=y` identifies `/dev/sda` instantaneously.
3. **RAM-First Preservation:** Windows partition is mounted read-only, `bifrost_blackbox.tar.zst` is copied to `/tmp/` in RAM, and Windows is unmounted before any disk-altering operations begin.

### Identified Bottlenecks:
1. **The "Demo Gap" (Missing Real Distro Rootfs):** Previously, `deploy_base_system` was called with `base_rootfs_archive: None`. While partitions and Btrfs subvolumes (`@root`, `@home`, `@blackbox`) were created, no actual Linux operating system (Kernel, `/usr/bin`, `pacman`, `systemd`) was installed. The system rebooted into an empty root subvolume.
2. **NTFS Shrink Brittleness:** Windows 11 NTFS partitions with active dirty flags, volume log checks, or fragmented tail clusters cause `ntfsresize` or `parted resizepart` to fail or reject shrinking.
3. **Control Character Terminal Clutter:** Unfiltered `\r` (carriage returns) from progress meters in subcommands caused Crossterm cursor position de-synchronization, scattering progress text across the TUI.

---

## 2. Strategic Directives for Argon

```
┌────────────────────────────────────────────────────────────────────────┐
│ WINDOWS RUNTIME (Phase 5: Packaging & Staging)                        │
├────────────────────────────────────────────────────────────────────────┤
│ 1. User selects Migration Strategy:                                    │
│    - [Mode A] Clean Install (Wipe Windows & Maximize Linux Space)      │
│    - [Mode B] Dual-Boot (Shrink NTFS & Preserve Windows Coexistence)   │
│ 2. Package Blackbox: `bifrost_blackbox.tar.zst`                        │
│ 3. Stage Target Distro Rootfs: e.g. `archlinux-bootstrap-x86_64.tar.zst`│
│ 4. Mount ESP & stage `bifrost-boot.efi`                                │
│ 5. Arm BCD BootSequence & trigger reboot                               │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │ Reboot (No BIOS keys required)
┌───────────────────────────────────▼────────────────────────────────────┐
│ BIFRÖST RAMFS ENVIRONMENT                                              │
├────────────────────────────────────────────────────────────────────────┤
│ 1. Mount Windows NTFS read-only (ro)                                   │
│ 2. Secure Blackbox & Distro Rootfs into RAM (`/tmp/`)                  │
│ 3. Unmount Windows partition (`umount /mnt/win_src`)                   │
│ 4. Partitioning Execution:                                             │
│    - Mode A (Clean): Re-partition entire disk for Btrfs (Zero NTFS risk)│
│    - Mode B (Dual):  Run `ntfsresize -ff` & `parted resizepart`        │
│ 5. Format Btrfs Subvolumes: `@root`, `@home`, `@blackbox`              │
│ 6. Extract Distro Rootfs into `@root` (Real Linux OS)                  │
│ 7. Inject Blackbox from RAM into `@home` (Enforce POSIX 0700/0600)     │
│ 8. Deploy systemd-boot to ESP & configure First-Boot Flatpak service   │
│ 9. Automated clean reboot directly into native Asgard!                 │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 3. Workstreams & Action Items

### Workstream 1: Migration Strategy Decision (Clean Install vs. Dual-Boot)
**Goal:** Empower the user to decide whether Windows should be wiped completely or kept alongside Linux.

- [ ] **Wizard UI Integration (`src/main.rs`):**
  - Add a dedicated selection step (or toggle in Blueprint/Flightplan):
    - `Option 1: Clean Install (Empfohlen)`: Replace Windows entirely. Dedicates 100% of the drive to Linux Btrfs.
    - `Option 2: Dual-Boot`: Shrink NTFS partition and install Linux in the freed space.
  - Record choice in `MigrationManifest` (`clean_install: bool`).
- [ ] **Clean-Install Partitioning Engine:**
  - When `clean_install == true`:
    - Do NOT run `ntfsresize` or `parted resizepart`.
    - Delete the Windows partition and create a single large Btrfs partition spanning the rest of the disk.
    - Formats effortlessly, eliminating all NTFS resize failures.

---

### Workstream 2: Real Distro Rootfs Staging & Deployment
**Goal:** Eliminate the prototype skeleton; install a genuine, bootable Arch Linux (or selected target distro) on `@root`.

- [ ] **Staging the Base Image in Windows:**
  - In Windows Phase 5/6, download or stage the minimal bootstrap image:
    - Arch Linux: `archlinux-bootstrap-x86_64.tar.zst` (~180 MB from official mirrors).
    - Save to `C:\bifrost_rootfs.tar.zst` alongside `bifrost_blackbox.tar.zst`.
- [ ] **Deploying Rootfs in RAMFS:**
  - Auto-discovery in RAMFS discovers both `bifrost_blackbox.tar.zst` and `bifrost_rootfs.tar.zst` and buffers them into `/tmp/`.
  - Pass the rootfs path to `deploy_base_system(&target_root, Some(&rootfs_path), ...)`.
  - Unpack the rootfs into `@root`.
  - Configure `/etc/fstab`, copy `/boot/vmlinuz-linux` and initramfs to ESP, and configure `systemd-boot`.

---

### Workstream 3: Bifröst Bridge Cinema UI & Design Autonomy
**Goal:** Deliver a breathtaking, glitch-free early boot visual experience.

- [ ] **Design Review Mandate for Argon:**
  - Review the current ASCII and TUI implementation in `src/bridge/ramfs_cinema.rs`.
  - **Argon has creative authority to adapt, refine, and improve the layout, typography, colors, and flow**, provided that **the core concept of an active, animated Bifröst Rainbow Bridge particle stream is preserved**.
- [ ] **Terminal Hygiene:**
  - Guarantee zero cursor distortion: sanitize all command outputs, stripping carriage returns (`\r`) and non-printable control characters.
  - Maintain the clean, minimalist header (isometric `BRIDGE` title, `THE BRIDGE TO ASGARD`, elapsed timer).

---

## 4. Definition of Done (DoD)
- [ ] User can explicitly choose between *Clean Install* (full wipe) and *Dual-Boot* (shrink).
- [ ] Blackbox is 100% preserved in RAM during Clean Install and restored to `/home`.
- [ ] A real base rootfs (Arch Linux) is extracted to `@root`, providing actual binaries (`/bin/sh`, `pacman`, `vmlinuz-linux`).
- [ ] The VM reboots directly into a working Linux shell or desktop without falling back to UEFI setup.

---

## 5. `bifroest-linux` (Zero-BIOS Linux-to-Linux Migration Engine)

Bifröst includes a dedicated static Linux binary (`bifroest-linux`, entry point `src/bin/bifroest_linux.rs`) sharing the `bifroest` core library (`src/lib.rs`) for direct Linux-to-Linux migrations:

- **Mode A – Btrfs Subvolume-Hop (`src/bridge/linux_hop.rs`):**
  - Reuses `@home` Zero-Copy without moving user files, installs the chosen distribution into `@root_<distro>`, preserves exact UID/GID/shadow credentials, LUKS2 `/etc/crypttab` + crypt hooks, and NetworkManager WiFi connections, and provisions kernel + chosen desktop (`CLI Only`, `XFCE4`, `KDE Plasma`, `GNOME`, `Hyprland`) in chroot (with host-kernel offline fallback).
  - **Hybrid Rollback Policy:** Users freely choose between keeping `@root_old` + `bifroest-rollback.conf` (later removable with `bifroest-linux --cleanup`) or **Direct Purge** after bootloader verification.
- **Mode B – RAMFS Full-Transit:**
  - Stages Blackbox + target rootfs + `bifrost-boot.efi` for Ext4/XFS/wipe transitions.
- **Zero-BIOS UEFI Handoff (`src/bridge/linux_efi.rs`):**
  - Configures `systemd-boot` entries and arms UEFI `efibootmgr --bootnext` without requiring BIOS interaction.
