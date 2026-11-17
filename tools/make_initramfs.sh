#!/bin/sh
set -e

export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"

OUT_FILE="${1:-initramfs-bifrost.cpio.gz}"
WORK_DIR="$(mktemp -d /tmp/bifrost_initramfs.XXXXXX)"
trap 'rm -rf "$WORK_DIR"' EXIT

echo "[Bifrost] Erstelle Initramfs-Verzeichnisstruktur in $WORK_DIR..."
for d in bin sbin etc proc sys dev run tmp mnt lib lib64 usr/bin usr/sbin usr/lib var/lib/bifrost; do
  mkdir -p "$WORK_DIR/$d"
done

copy_with_libs() {
  BIN_NAME="$1"
  BIN_PATH="$(command -v "$BIN_NAME" || true)"
  if [ -z "$BIN_PATH" ]; then
    echo "[WARN] Tool '$BIN_NAME' nicht gefunden - überspringe."
    return
  fi
  DEST_BIN="$WORK_DIR/bin/$(basename "$BIN_PATH")"
  cp -L "$BIN_PATH" "$DEST_BIN"
  chmod +x "$DEST_BIN"
  ldd "$BIN_PATH" 2>/dev/null | grep -o '/[^ ]*' | while read -r lib; do
    if [ -f "$lib" ]; then
      DEST_DIR="$WORK_DIR$(dirname "$lib")"
      mkdir -p "$DEST_DIR"
      cp -L "$lib" "$DEST_DIR/"
    fi
  done
}

# Statisches Rust-Binary + BusyBox + Partitionierungs-/FS-Werkzeuge
if [ -x /bin/bifroest ]; then
  cp /bin/bifroest "$WORK_DIR/bin/bifroest"
elif [ -x ./target/x86_64-unknown-linux-musl/release/bifroest ]; then
  cp ./target/x86_64-unknown-linux-musl/release/bifroest "$WORK_DIR/bin/bifroest"
elif [ -x ./target/release/bifroest ]; then
  cp ./target/release/bifroest "$WORK_DIR/bin/bifroest"
else
  echo "[ERROR] Statisches bifroest Binary nicht gefunden!"
  exit 1
fi

if [ -x /bin/busybox.static ]; then
  cp /bin/busybox.static "$WORK_DIR/bin/busybox"
else
  copy_with_libs busybox
fi
chmod +x "$WORK_DIR/bin/busybox"

for tool in ntfsresize ntfsfix ntfs-3g mount.ntfs-3g mkfs.btrfs btrfs mkfs.ext4 parted partprobe sfdisk lsblk blkid modprobe depmod; do
  copy_with_libs "$tool"
done

# Sicherstellen, dass der musl dynamic linker vorhanden ist (falls ldd ihn übersprang)
if [ -f /lib/ld-musl-x86_64.so.1 ] && [ ! -f "$WORK_DIR/lib/ld-musl-x86_64.so.1" ]; then
  cp -L /lib/ld-musl-x86_64.so.1 "$WORK_DIR/lib/"
fi

# Kernel-Module für Hyper-V Gen 2, VirtIO, NVMe, SCSI und Dateisysteme mitnehmen
KVER="$(ls -1 /lib/modules 2>/dev/null | head -n 1 || true)"
if [ -n "$KVER" ] && [ -d "/lib/modules/$KVER" ]; then
  echo "[Bifrost] Kopiere Kernel-Module für $KVER..."
  MOD_SRC="/lib/modules/$KVER"
  MOD_DST="$WORK_DIR/lib/modules/$KVER"
  mkdir -p "$MOD_DST"

  for subdir in \
    kernel/drivers/hv \
    kernel/drivers/hid \
    kernel/drivers/input \
    kernel/drivers/scsi \
    kernel/drivers/ata \
    kernel/drivers/nvme \
    kernel/drivers/block \
    kernel/drivers/virtio \
    kernel/drivers/video \
    kernel/drivers/gpu \
    kernel/fs \
    kernel/crypto \
    kernel/lib; do
    if [ -d "$MOD_SRC/$subdir" ]; then
      mkdir -p "$MOD_DST/$(dirname "$subdir")"
      cp -a "$MOD_SRC/$subdir" "$MOD_DST/$(dirname "$subdir")/"
    fi
  done

  for meta in modules.order modules.builtin modules.builtin.modinfo; do
    [ -f "$MOD_SRC/$meta" ] && cp "$MOD_SRC/$meta" "$MOD_DST/"
  done

  if command -v depmod >/dev/null 2>&1; then
    depmod -b "$WORK_DIR" "$KVER" || true
  fi
else
  echo "[WARN] Keine Kernel-Module unter /lib/modules gefunden."
fi

# Symlinks für BusyBox Applets (bestehende echte Binaries wie modprobe nicht überschreiben)
for cmd in sh mount umount mdev insmod mkdir find head cp rm cat echo grep sleep clear poweroff reboot ls yes dd df chmod chown chroot cttyhack setsid; do
  if [ ! -e "$WORK_DIR/bin/$cmd" ]; then
    ln -sf /bin/busybox "$WORK_DIR/bin/$cmd"
  fi
done
for cmd in mdev modprobe insmod switch_root; do
  if [ ! -e "$WORK_DIR/sbin/$cmd" ]; then
    if [ -x "$WORK_DIR/bin/$cmd" ]; then
      ln -sf "/bin/$cmd" "$WORK_DIR/sbin/$cmd"
    else
      ln -sf /bin/busybox "$WORK_DIR/sbin/$cmd"
    fi
  fi
done

# Minimal /etc files for chroot/systemd-boot & user mapping
echo "root:x:0:0:root:/root:/bin/sh" > "$WORK_DIR/etc/passwd"
echo "root:x:0:" > "$WORK_DIR/etc/group"
echo "bifroest-ramfs" > "$WORK_DIR/etc/hostname"

# Static device nodes for guaranteed early console
mkdir -p "$WORK_DIR/dev"
mknod -m 600 "$WORK_DIR/dev/console" c 5 1 2>/dev/null || true
mknod -m 666 "$WORK_DIR/dev/tty" c 5 0 2>/dev/null || true
mknod -m 620 "$WORK_DIR/dev/tty0" c 4 0 2>/dev/null || true
mknod -m 620 "$WORK_DIR/dev/tty1" c 4 1 2>/dev/null || true
mknod -m 666 "$WORK_DIR/dev/null" c 1 3 2>/dev/null || true
mknod -m 666 "$WORK_DIR/dev/zero" c 1 5 2>/dev/null || true

# Early-Boot /init Skript (läuft als PID 1 in der VM / im RAMFS)
cat << 'EOF' > "$WORK_DIR/init"
#!/bin/sh
export PATH=/bin:/sbin:/usr/bin:/usr/sbin
export TERM=linux

mount -t proc none /proc
mount -t sysfs none /sys
mount -t devtmpfs none /dev || true
mkdir -p /dev/pts /run /tmp /mnt/win_src /mnt/target_linux
mount -t tmpfs none /run || true
mount -t tmpfs none /tmp || true

# Dual logging to both stdout/console AND /dev/kmsg (guaranteed display on earlycon=efifb)
kmsg_log() {
  echo "<6>[Bifrost] $1" > /dev/kmsg 2>/dev/null || true
  echo "[Bifrost] $1"
}

if [ -c /dev/tty0 ]; then
  exec </dev/tty0 >/dev/tty0 2>&1
elif [ -c /dev/console ]; then
  exec </dev/console >/dev/console 2>&1
fi

kmsg_log "============================================="
kmsg_log "=== Bifrost Linux RAMFS Migration Kernel ==="
kmsg_log "============================================="
kmsg_log "Lade Storage-, Hyper-V- und Dateisystem-Treiber..."

for mod in hv_vmbus hyperv_keyboard hyperv_drm hv_storvsc hv_utils hid hid_hyperv atkbd serio evdev scsi_mod sd_mod nvme virtio_pci virtio_blk virtio_scsi btrfs vfat fat nls_cp437 nls_iso8859_1 fuse ntfs3; do
  modprobe "$mod" 2>/dev/null || true
done

mdev -s 2>/dev/null || true
sleep 2
mdev -s 2>/dev/null || true

if grep -q "bifrost_auto=1" /proc/cmdline; then
  kmsg_log "Suche bifrost_blackbox.tar.zst auf lokalen Partitionen..."
  ARCHIVE_RAM="/tmp/bifrost_blackbox.tar.zst"
  WIN_PART=""

  for part in /dev/sd[a-z][0-9]* /dev/vd[a-z][0-9]* /dev/nvme[0-9]n[0-9]p[0-9]*; do
    [ -b "$part" ] || continue
    kmsg_log "Prüfe Partition $part..."

    # Reset any NTFS dirty or hibernation flags so ntfs-3g mounts cleanly
    ntfsfix -d "$part" >/dev/null 2>&1 || true

    if ntfs-3g -o ro "$part" /mnt/win_src 2>/dev/null || mount -t ntfs3 -o ro "$part" /mnt/win_src 2>/dev/null || mount -o ro "$part" /mnt/win_src 2>/dev/null; then
      FOUND=$(find /mnt/win_src -name "bifrost_blackbox.tar.zst" 2>/dev/null | head -n 1 || true)
      if [ -n "$FOUND" ]; then
        kmsg_log "Blackbox gefunden auf $part: $FOUND"
        kmsg_log "Kopiere Blackbox in den RAM ($ARCHIVE_RAM)..."
        cp "$FOUND" "$ARCHIVE_RAM"
        WIN_PART="$part"
        umount /mnt/win_src || true
        break
      fi
      umount /mnt/win_src || true
    fi
  done

  if [ -n "$WIN_PART" ] && [ -f "$ARCHIVE_RAM" ]; then
    kmsg_log "Bereinige NTFS Dirty-Flag auf $WIN_PART vor Shrink..."
    ntfsfix -d "$WIN_PART" >/dev/null 2>&1 || true

    kmsg_log "Starte Bifröst RAMFS Bridge Engine..."
    # Stream every output line of bifroest directly to /dev/kmsg so user sees 100% progress!
    if /bin/bifroest --ramfs --archive "$ARCHIVE_RAM" --target-root /mnt/target_linux 2>&1 | while IFS= read -r line; do
      echo "<6>[Bifrost] $line" > /dev/kmsg 2>/dev/null || true
      echo "$line"
    done; then
      kmsg_log "=== Bifrost Migration erfolgreich abgeschlossen! ==="
      kmsg_log "Synchronisiere Basis-Runtime in @root (/mnt/target_linux)..."
      mkdir -p /mnt/target_linux/bin /mnt/target_linux/sbin /mnt/target_linux/lib /mnt/target_linux/usr /mnt/target_linux/proc /mnt/target_linux/sys /mnt/target_linux/dev /mnt/target_linux/tmp /mnt/target_linux/run
      cp -a /bin/. /mnt/target_linux/bin/ 2>/dev/null || true
      cp -a /sbin/. /mnt/target_linux/sbin/ 2>/dev/null || true
      cp -a /lib/. /mnt/target_linux/lib/ 2>/dev/null || true
      kmsg_log "=== Migration abgeschlossen! Neustart in 5 Sekunden... ==="
      sleep 5
      reboot -f 2>/dev/null || sync && reboot -f
    else
      kmsg_log "[ERROR] Bifröst RAMFS Engine meldete einen Fehler!"
      exec setsid cttyhack /bin/sh 2>/dev/null || exec /bin/sh
    fi
  else
    kmsg_log "[WARN] Keine bifrost_blackbox.tar.zst auf lokalen Partitionen gefunden."
    kmsg_log "[Init] Starte interaktive Bifröst TUI..."
  fi
fi

setsid cttyhack /bin/bifroest </dev/tty0 >/dev/tty0 2>&1 || /bin/bifroest </dev/tty0 >/dev/tty0 2>&1 || true
exec setsid cttyhack /bin/sh 2>/dev/null || exec /bin/sh
EOF

chmod +x "$WORK_DIR/init" "$WORK_DIR/bin/bifroest"

echo "[Bifrost] Packe CPIO + Gzip Archiv nach $OUT_FILE..."
(cd "$WORK_DIR" && find . -print0 | cpio --null -ov --format=newc | gzip -9) > "$OUT_FILE"
echo "[OK] Initramfs erstellt: $OUT_FILE ($(du -h "$OUT_FILE" | cut -f1))"
