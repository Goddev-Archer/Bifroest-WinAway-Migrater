#!/bin/sh
set -e

OUT_FILE="${1:-initramfs-bifrost.cpio.gz}"
WORK_DIR="$(mktemp -d /tmp/bifrost_initramfs.XXXXXX)"
trap 'rm -rf "$WORK_DIR"' EXIT

echo "[Bifrost] Erstelle Initramfs-Verzeichnisstruktur in $WORK_DIR..."
mkdir -p "$WORK_DIR"/{bin,sbin,etc,proc,sys,dev,mnt,lib,lib64,usr/bin,usr/sbin,var/lib/bifrost}

copy_with_libs() {
  BIN_NAME="$1"
  BIN_PATH="$(command -v "$BIN_NAME" || true)"
  if [ -z "$BIN_PATH" ]; then
    echo "[WARN] Tool '$BIN_NAME' nicht gefunden - überspringe."
    return
  fi
  cp -L "$BIN_PATH" "$WORK_DIR/bin/$(basename "$BIN_PATH")"
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
else
  echo "[ERROR] Statisches bifroest Binary nicht gefunden!"
  exit 1
fi

copy_with_libs busybox
copy_with_libs ntfsresize
copy_with_libs ntfs-3g
copy_with_libs mkfs.btrfs
copy_with_libs btrfs
copy_with_libs mkfs.ext4
copy_with_libs parted
copy_with_libs partprobe
copy_with_libs sfdisk
copy_with_libs lsblk
copy_with_libs blkid

# Symlinks für BusyBox Applets
for cmd in sh mount umount mkdir find head sleep clear poweroff reboot ls cat echo grep; do
  ln -sf /bin/busybox "$WORK_DIR/bin/$cmd"
done

# Early-Boot /init Skript (läuft auf /dev/tty1 in der VM / RAMFS)
cat << 'EOF' > "$WORK_DIR/init"
#!/bin/sh
export PATH=/bin:/sbin:/usr/bin:/usr/sbin
export TERM=linux

mount -t proc none /proc
mount -t sysfs none /sys
mount -t devtmpfs none /dev || true

clear
echo "=== Bifrost Linux RAMFS Migration Kernel ==="

if grep -q "bifrost_auto=1" /proc/cmdline; then
  mkdir -p /mnt/win_src /mnt/target_linux
  for part in /dev/vda3 /dev/sda3 /dev/nvme0n1p3; do
    if [ -b "$part" ] && ntfs-3g -o ro "$part" /mnt/win_src 2>/dev/null; then
      FOUND=$(find /mnt/win_src/Users -maxdepth 3 -name bifrost_blackbox.tar.zst 2>/dev/null | head -n 1)
      if [ -n "$FOUND" ]; then
        exec /bin/bifroest --ramfs --archive "$FOUND" --target-root /mnt/target_linux
      fi
      umount /mnt/win_src || true
    fi
  done
fi

exec /bin/bifroest </dev/tty1 >/dev/tty1 2>&1
EOF

chmod +x "$WORK_DIR/init" "$WORK_DIR/bin/bifroest"

echo "[Bifrost] Packe CPIO + Gzip Archiv nach $OUT_FILE..."
(cd "$WORK_DIR" && find . -print0 | cpio --null -ov --format=newc | gzip -9) > "$OUT_FILE"
echo "[OK] Initramfs erstellt: $OUT_FILE"
