# AGENTS.md – Directives for Argon: Zero-BIOS-Interaction Migration Pipeline

> **Role & Assignment:** Argon (Orchestration & Systems Integration Agent)  
> **Mission:** Transform Bifröst from an interactive dry-run tool into a **fully automated, zero-BIOS, zero-USB, zero-DVD in-place migration engine** from Windows 11 to Linux.  
> **Core Architectural Principle:** The user must NEVER be forced to enter BIOS/UEFI settings, burn an ISO, or insert physical boot media. The entire handoff must happen programmatically from within Windows.

---

## 1. Incident & Root Cause Analysis (Status Quo)

### What Happened in v0.1.0:
1. The user executed `bifroest.exe` inside the Tiny11 test VM (`Test-Computer`).
2. Hardware inspection, disk scanner, candidate selection, and blackbox packaging (`bifrost_blackbox.tar.zst`) completed successfully.
3. The user executed `stage_efi_boot.bat` and rebooted the VM.
4. **Failure:** The VM booted straight back into the Windows 11 login screen (`RemoveThisShittySys`).

### Root Cause:
1. **Unarmed BCD Scripts:** `src/bridge/handoff.rs` generated a template batch script where the critical BCD activation commands were commented out with `rem` (`rem bcdedit /set {NEW_GUID} ...`, `rem bcdedit /bootsequence ...`).
2. **Missing Migration Kernel & Initramfs:** Neither `vmlinuz-bifrost.efi` nor `initramfs-bifrost.img` were downloaded, generated, or copied to the EFI System Partition (ESP). Windows Boot Manager found no valid alternate OS and booted default Windows.
3. **Manual Human Burden:** Expecting the user to copy-paste BCD GUIDs or manually adjust UEFI settings breaks the core UX promise of Bifröst.

---

## 2. Target Architecture: Unified Kernel Image (UKI) + Automated BCD Arming

```
┌────────────────────────────────────────────────────────────────────────┐
│ WINDOWS RUNTIME (Phase 5: Executing in `bifroest.exe`)                 │
├────────────────────────────────────────────────────────────────────────┤
│ 1. Mount ESP silently: `mountvol S: /S`                                │
│ 2. Stage UKI: Deploy `bifrost-boot.efi` into `S:\EFI\Bifrost\`         │
│ 3. Arm One-Time Boot: Parse GUID & execute `bcdedit /bootsequence`     │
│ 4. Request system reboot                                               │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │ Reboot (No BIOS keys required)
┌───────────────────────────────────▼────────────────────────────────────┐
│ UEFI FIRMWARE DISPATCH                                                 │
├────────────────────────────────────────────────────────────────────────┤
│ UEFI Firmware reads BootSequence -> executes `S:\EFI\Bifrost\bifrost-boot.efi` │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │ Single PE/COFF Executable boots into RAM
┌───────────────────────────────────▼────────────────────────────────────┐
│ BIFRÖST RAMFS ENVIRONMENT                                              │
├────────────────────────────────────────────────────────────────────────┤
│ - Linux Kernel + Initramfs + Embedded Cmdline (`bifrost_auto=1`)       │
│ - Runs `bifroest --ramfs` as PID 1                                     │
│ - Locates `bifrost_blackbox.tar.zst` on Windows NTFS                   │
│ - Executes non-destructive `ntfsresize`                                │
│ - Creates Btrfs subvolumes (`@root`, `@home`, `@blackbox`)             │
│ - Extracts Blackbox + enforces POSIX mode (0700/0600 on `.ssh`)        │
│ - Deploys systemd-boot to ESP & configures first-boot Flatpak service  │
│ - Final reboot directly into new native Linux!                         │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 3. Workstreams & Action Items for Argon

### Workstream 1: Unified Kernel Image (UKI) Generation
**Goal:** Produce a single, self-contained `bifrost-boot.efi` binary (~25-35 MB) that contains the Linux kernel, RAMFS, and hardcoded command line.

- [ ] **Cross-Compile Static Linux Binary:**
  - Build `bifroest` targeting `x86_64-unknown-linux-musl` with `--features ramfs` or standalone.
  - Zero dynamic libc dependencies.
- [ ] **Assemble Minimal CPIO Initramfs:**
  - Incorporate BusyBox (static), `ntfs-3g`, `ntfsresize`, `mkfs.btrfs`, `parted`, `sfdisk`, and static `bifroest`.
  - Include `/init` script that automatically mounts `/proc`, `/sys`, `/dev` and invokes `/bin/bifroest --ramfs`.
- [ ] **Combine into UKI (`bifrost-boot.efi`):**
  - Use `ukify` or `objcopy` with systemd-stub (or Alpine minimal EFI kernel with embedded initramfs):
    ```bash
    objcopy \
      --add-section .osrel=/etc/os-release --change-section-vma .osrel=0x20000 \
      --add-section .cmdline=cmdline.txt --change-section-vma .cmdline=0x30000 \
      --add-section .linux=vmlinuz --change-section-vma .linux=0x2000000 \
      --add-section .initrd=initramfs.cpio.gz --change-section-vma .initrd=0x3000000 \
      linuxx64.efi.stub bifrost-boot.efi
    ```
- [ ] **Release Asset Packaging:**
  - Publish `bifrost-boot.efi` as a standardized release asset on GitHub (`https://github.com/Goddev-Archer/Bifroest-WinAway-Migrater/releases`).

---

### Workstream 2: Automated Windows BCD & EFI Staging Engine
**Goal:** Completely eliminate manual batch editing; `bifroest.exe` must autonomously arm the UEFI bootloader before asking the user to reboot.

- [ ] **Update `src/bridge/handoff.rs`:**
  - Implement programmatic ESP mounting:
    - Check if drive `S:` is free; find next free letter if taken.
    - Run `mountvol S: /S`.
  - Implement UKI deployment:
    - Download or unpack `bifrost-boot.efi` into `S:\EFI\Bifrost\bifrost-boot.efi`.
  - Implement Automated BCD Registration & Parsing:
    - Execute:
      ```cmd
      bcdedit /copy {bootmgr} /d "Bifrost Linux Bridge (RAMFS)"
      ```
    - Parse the returned GUID using regex (`\{[a-fA-F0-9\-]+\}`).
    - Immediately execute without human intervention:
      ```cmd
      bcdedit /set {GUID} path \EFI\Bifrost\bifrost-boot.efi
      bcdedit /bootsequence {GUID}
      ```
  - Implement Safety Validation:
    - Verify that `S:\EFI\Bifrost\bifrost-boot.efi` exists and is non-empty before calling `bcdedit /bootsequence`.
    - Detect Secure Boot state (`Confirm-SecureBootUEFI` / registry) and warn if unsigned UKI would be blocked by Hyper-V / firmware.

---

### Workstream 3: End-to-End VM Verification Protocol
**Goal:** Test and verify the full unbroken chain inside the Tiny11 Hyper-V VM.

1. **Pre-flight Check:**
   - Host drive `C:\` must maintain at least 8-10 GB free space (prevent VM auto-pause).
   - In Hyper-V VM settings: Set Secure Boot to *Microsoft UEFI Certificate Authority* (or disabled for dev testing).
2. **Execute In-VM:**
   - Run `bifroest.exe` in VM.
   - Choose Linux distro & target Btrfs subvolumes.
   - Click *ZÜNDUNG / PACKEN*.
   - Confirm Blackbox is generated and UKI is staged to `S:\EFI\Bifrost\bifrost-boot.efi`.
3. **Reboot:**
   - Initiate reboot.
   - **Acceptance Criteria:** VM must NOT load Windows, but immediately display `=== Bifrost Linux RAMFS Migration Kernel ===`.
4. **Post-Migration Verification:**
   - VM boots into new Btrfs root.
   - `bifrost_blackbox.tar.zst` safely restored to `/home`.
   - File permissions: `.ssh` is `0700`, keys are `0600`.
   - Desktop and terminal are fully operational.

---

## 4. Definition of Done (DoD)
- [ ] No manual batch file editing or GUID copy-pasting is required by the user.
- [ ] No BIOS/UEFI boot menu intervention is needed.
- [ ] `bifroest.exe` arms the one-time boot sequence automatically.
- [ ] The VM transitions from Windows directly into the Linux RAMFS and finalizes the installation.
