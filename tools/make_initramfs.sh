#!/bin/sh
set -e

export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"

OUT_FILE="${1:-initramfs-bifrost.cpio.gz}"
WORK_DIR="$(mktemp -d /tmp/bifrost_initramfs.XXXXXX)"
ROOTFS_STAGE="$(mktemp -d /tmp/bifrost_rootfs_stage.XXXXXX)"
trap 'rm -rf "$WORK_DIR" "$ROOTFS_STAGE"' EXIT

echo "[Bifrost] Erstelle Initramfs-Verzeichnisstruktur in $WORK_DIR..."
for d in bin sbin etc proc sys dev run tmp mnt lib lib64 usr/bin usr/sbin usr/lib usr/share/bifrost var/lib/bifrost; do
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

# BusyBox udhcpc Konfigurationsskript (setzt IPv4, Gateway & DNS im RAMFS und im Zielsystem)
mkdir -p "$WORK_DIR/usr/share/udhcpc"
cat << 'EOS' > "$WORK_DIR/usr/share/udhcpc/default.script"
#!/bin/sh
case "$1" in
  deconfig)
    ip addr flush dev "$interface" 2>/dev/null || true
    ip link set dev "$interface" up 2>/dev/null || true
    ;;
  bound|renew)
    ip addr add "$ip/${mask:-24}" dev "$interface" 2>/dev/null || true
    if [ -n "$router" ]; then
      for r in $router; do
        ip route replace default via "$r" dev "$interface" 2>/dev/null && break
      done
    fi
    : > /etc/resolv.conf
    for d in $dns; do
      echo "nameserver $d" >> /etc/resolv.conf
    done
    [ -s /etc/resolv.conf ] || echo "nameserver 1.1.1.1" > /etc/resolv.conf
    ;;
esac
exit 0
EOS
chmod +x "$WORK_DIR/usr/share/udhcpc/default.script"

# Bundled base Linux rootfs (/usr/share/bifrost/base-rootfs.tar.zst) BEFORE copying heavy kernel modules
if command -v zstd >/dev/null 2>&1; then
  echo "[Bifrost] Erstelle gebündeltes Basis-Rootfs (/usr/share/bifrost/base-rootfs.tar.zst)..."
  for d in bin sbin usr/bin usr/sbin usr/lib usr/local/bin usr/local/sbin usr/share/udhcpc lib lib64 etc etc/profile.d etc/apk/keys etc/ssl/certs lib/apk/db var/lib/bifrost var/cache/apk var/log root home proc sys dev dev/pts dev/shm run tmp mnt boot/efi; do
    mkdir -p "$ROOTFS_STAGE/$d"
  done
  chmod 1777 "$ROOTFS_STAGE/tmp"
  ln -sf ../run "$ROOTFS_STAGE/var/run"

  cp -a "$WORK_DIR/bin/." "$ROOTFS_STAGE/bin/"
  cp -a "$WORK_DIR/lib/." "$ROOTFS_STAGE/lib/" 2>/dev/null || true
  cp -a "$WORK_DIR/usr/lib/." "$ROOTFS_STAGE/usr/lib/" 2>/dev/null || true
  cp -a "$WORK_DIR/usr/share/udhcpc/default.script" "$ROOTFS_STAGE/usr/share/udhcpc/default.script"
  chmod +x "$ROOTFS_STAGE/usr/share/udhcpc/default.script"

  # Vorinstalliertes apk.static + Alpine v3.20 Keys/Certs + initialisierte APK-Datenbank
  if [ -x /sbin/apk.static ]; then
    cp /sbin/apk.static "$ROOTFS_STAGE/sbin/apk.static"
    chmod +x "$ROOTFS_STAGE/sbin/apk.static"
    ln -sf /sbin/apk.static "$ROOTFS_STAGE/sbin/apk"
    cp -a /etc/apk/keys/. "$ROOTFS_STAGE/etc/apk/keys/" 2>/dev/null || true
    cp -a /etc/ssl/certs/. "$ROOTFS_STAGE/etc/ssl/certs/" 2>/dev/null || true
    cat << 'EOS' > "$ROOTFS_STAGE/etc/apk/repositories"
https://dl-cdn.alpinelinux.org/alpine/v3.20/main
https://dl-cdn.alpinelinux.org/alpine/v3.20/community
EOS
    /sbin/apk.static --root "$ROOTFS_STAGE" --initdb add >/dev/null 2>&1 || \
      touch "$ROOTFS_STAGE/lib/apk/db/installed" "$ROOTFS_STAGE/lib/apk/db/lock" "$ROOTFS_STAGE/lib/apk/db/scripts.tar" "$ROOTFS_STAGE/lib/apk/db/triggers"
  fi

  for applet in $("$WORK_DIR/bin/busybox" --list 2>/dev/null || true); do
    [ -n "$applet" ] || continue
    if [ ! -e "$ROOTFS_STAGE/bin/$applet" ]; then
      ln -sf /bin/busybox "$ROOTFS_STAGE/bin/$applet"
    fi
  done
  for scmd in init mdev modprobe insmod poweroff reboot halt switch_root getty login udhcpc; do
    if [ ! -e "$ROOTFS_STAGE/sbin/$scmd" ]; then
      ln -sf /bin/busybox "$ROOTFS_STAGE/sbin/$scmd"
    fi
  done

  mknod -m 600 "$ROOTFS_STAGE/dev/console" c 5 1 2>/dev/null || true
  mknod -m 666 "$ROOTFS_STAGE/dev/tty" c 5 0 2>/dev/null || true
  mknod -m 620 "$ROOTFS_STAGE/dev/tty0" c 4 0 2>/dev/null || true
  mknod -m 620 "$ROOTFS_STAGE/dev/tty1" c 4 1 2>/dev/null || true
  mknod -m 666 "$ROOTFS_STAGE/dev/null" c 1 3 2>/dev/null || true
  mknod -m 666 "$ROOTFS_STAGE/dev/zero" c 1 5 2>/dev/null || true

  cat << 'EOS' > "$ROOTFS_STAGE/etc/passwd"
root:x:0:0:root:/root:/bin/sh
EOS
  cat << 'EOS' > "$ROOTFS_STAGE/etc/group"
root:x:0:
tty:x:5:
disk:x:6:
lp:x:7:
kmem:x:9:
wheel:x:10:root
audio:x:18:
cdrom:x:19:
dialout:x:20:
video:x:27:
users:x:100:
input:x:101:
EOS
  echo "asgard-linux" > "$ROOTFS_STAGE/etc/hostname"
  cat << 'EOS' > "$ROOTFS_STAGE/etc/os-release"
NAME="Alpine Linux"
PRETTY_NAME="Alpine Linux (Bifrost Asgard Minimal)"
ID=alpine
ID_LIKE=alpine
EOS
  cat << 'EOS' > "$ROOTFS_STAGE/etc/inittab"
::sysinit:/bin/mount -o remount,rw /
::sysinit:/bin/mkdir -p /dev/pts /dev/shm /run
::sysinit:/bin/mount -t devpts devpts /dev/pts
::sysinit:/bin/mount -t tmpfs tmpfs /dev/shm
::once:/bin/mdev -s
tty1::respawn:/bin/sh -l
::ctrlaltdel:/sbin/reboot
::shutdown:/bin/umount -a -r
EOS
  cat << 'EOS' > "$ROOTFS_STAGE/etc/profile"
export PATH=/usr/local/sbin:/usr/local/bin:/bin:/sbin:/usr/bin:/usr/sbin
export TERM=linux
export PS1='[\u@asgard-linux \W]\$ '
for f in /etc/profile.d/*.sh; do
  [ -r "$f" ] && . "$f"
done
EOS

  tar -C "$ROOTFS_STAGE" -cf - . | zstd -T0 -19 -q -o "$WORK_DIR/usr/share/bifrost/base-rootfs.tar.zst"
  echo "[OK] Basis-Rootfs erstellt: $(du -h "$WORK_DIR/usr/share/bifrost/base-rootfs.tar.zst" | cut -f1)"
  rm -rf "$ROOTFS_STAGE"
else
  echo "[WARN] zstd nicht gefunden - überspringe base-rootfs.tar.zst Bündelung."
fi

# Kernel-Module für Hyper-V Gen 2, VirtIO, NVMe, SCSI und Dateisysteme mitnehmen
KVER="$(ls -1 /lib/modules 2>/dev/null | grep -E 'virt|lts' | head -n 1 || ls -1 /lib/modules 2>/dev/null | head -n 1 || true)"
if [ -n "$KVER" ] && [ -d "/lib/modules/$KVER" ]; then
  echo "[Bifrost] Kopiere Kernel-Module für $KVER..."
  MOD_SRC="/lib/modules/$KVER"
  MOD_DST="$WORK_DIR/lib/modules/$KVER"
  mkdir -p "$MOD_DST"
  cp -a "$MOD_SRC"/. "$MOD_DST/"

  if command -v depmod >/dev/null 2>&1; then
    depmod -b "$WORK_DIR" "$KVER" || true
  fi
else
  echo "[WARN] Keine Kernel-Module unter /lib/modules gefunden."
fi

# Symlinks für BusyBox Applets (bestehende echte Binaries wie modprobe nicht überschreiben)
for cmd in sh mount umount mdev insmod mkdir find head tail cp mv rm cat echo grep awk cut tr sleep clear poweroff reboot ls yes dd df du chmod chown chroot cttyhack setsid sync uname ip udhcpc ln; do
  if [ ! -e "$WORK_DIR/bin/$cmd" ]; then
    ln -sf /bin/busybox "$WORK_DIR/bin/$cmd"
  fi
done
for cmd in mdev modprobe insmod switch_root init udhcpc; do
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
mknod -m 666 "$WORK_DIR/dev/fuse" c 10 229 2>/dev/null || true

# Early-Boot /init Skript (läuft als PID 1 in der VM / im RAMFS)
cat << 'EOF' > "$WORK_DIR/init"
#!/bin/sh
export PATH=/bin:/sbin:/usr/bin:/usr/sbin
export TERM=linux

mount -t proc none /proc
mount -t sysfs none /sys
mount -t devtmpfs none /dev || true
mkdir -p /dev/pts /dev/shm /run /tmp /mnt/win_src /mnt/target_linux
mount -t devpts devpts /dev/pts 2>/dev/null || true
mount -t tmpfs tmpfs /dev/shm 2>/dev/null || true
mount -t tmpfs none /run || true
mount -t tmpfs none /tmp || true
echo 1 > /proc/sys/kernel/sysrq 2>/dev/null || true

# ACPI & Signal Handler für sauberes Herunterfahren/Neustarten aus Hyper-V
trap 'poweroff -f 2>/dev/null || echo o > /proc/sysrq-trigger' PWR INT TERM HUP

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

for mod in hv_vmbus hyperv_keyboard hyperv_drm hv_storvsc hv_utils hv_netvsc hid hid_hyperv atkbd serio evdev scsi_mod sd_mod nvme virtio_pci virtio_blk virtio_scsi virtio_net btrfs ext4 vfat fat nls_cp437 nls_iso8859_1 fuse ntfs3; do
  modprobe "$mod" 2>/dev/null || true
done

[ -c /dev/fuse ] || mknod -m 666 /dev/fuse c 10 229 2>/dev/null || true

kmsg_log "Warte auf Initialisierung der Festplatten (SCSI/VMBus)..."
for i in 1 2 3 4 5 6 7 8 9 10; do
  mdev -s 2>/dev/null || true
  PARTS=$(ls -1 /dev/sd[a-z][0-9]* /dev/vd[a-z][0-9]* /dev/nvme[0-9]n[0-9]p[0-9]* 2>/dev/null || true)
  if [ -n "$PARTS" ]; then
    kmsg_log "Partitionen nach ${i}s gefunden: $PARTS"
    break
  fi
  sleep 1
done

bring_up_network() {
  for netmod in hv_netvsc virtio_net e1000 e1000e r8169 igb; do
    modprobe "$netmod" 2>/dev/null || true
  done
  ip link set lo up 2>/dev/null || true
  for ifpath in /sys/class/net/*; do
    [ -e "$ifpath" ] || continue
    ifname="${ifpath##*/}"
    [ "$ifname" = "lo" ] && continue
    kmsg_log "Aktiviere Netzwerk-Interface $ifname (DHCP)..."
    ip link set "$ifname" up 2>/dev/null || true
    udhcpc -t 5 -T 1 -n -q -i "$ifname" -s /usr/share/udhcpc/default.script >/dev/null 2>&1 || true
    if ip -4 addr show dev "$ifname" 2>/dev/null | grep -q "inet "; then
      kmsg_log "Netzwerk aktiv auf $ifname."
      break
    fi
  done
}

install_target_runtime_helpers() {
  TGT="$1"
  KREL="$(uname -r 2>/dev/null || true)"
  if [ -n "$KREL" ] && [ -d "/lib/modules/$KREL" ] && [ ! -d "$TGT/lib/modules/$KREL" ]; then
    mkdir -p "$TGT/lib/modules"
    cp -a "/lib/modules/$KREL" "$TGT/lib/modules/" 2>/dev/null || true
  fi
  mkdir -p "$TGT/usr/local/bin" "$TGT/usr/local/sbin" "$TGT/usr/share/udhcpc"
  if [ -x /bin/busybox ] && [ ! -x "$TGT/usr/local/bin/busybox" ]; then
    cp /bin/busybox "$TGT/usr/local/bin/busybox" 2>/dev/null || true
    chmod +x "$TGT/usr/local/bin/busybox" 2>/dev/null || true
  fi
  if [ -x /usr/share/udhcpc/default.script ] && [ ! -x "$TGT/usr/share/udhcpc/default.script" ]; then
    cp /usr/share/udhcpc/default.script "$TGT/usr/share/udhcpc/default.script" 2>/dev/null || true
    chmod +x "$TGT/usr/share/udhcpc/default.script" 2>/dev/null || true
  fi
  for pcmd in reboot poweroff; do
    if [ ! -e "$TGT/sbin/$pcmd" ] && [ ! -e "$TGT/usr/bin/$pcmd" ] && [ ! -e "$TGT/usr/local/sbin/$pcmd" ]; then
      ln -sf /usr/local/bin/busybox "$TGT/usr/local/sbin/$pcmd" 2>/dev/null || true
    fi
  done
}

if grep -q "bifrost_auto=1" /proc/cmdline; then
  kmsg_log "Suche bifrost_blackbox.tar.zst auf Windows/NTFS-Partitionen..."
  ARCHIVE_RAM="/tmp/bifrost_blackbox.tar.zst"
  ROOTFS_RAM="/tmp/bifrost_rootfs.tar.zst"
  WIN_PART=""

  for part in /dev/sd[a-z][0-9]* /dev/vd[a-z][0-9]* /dev/nvme[0-9]n[0-9]p[0-9]*; do
    [ -b "$part" ] || continue
    FSTYPE=$(blkid -s TYPE -o value "$part" 2>/dev/null || true)
    if [ "$FSTYPE" = "btrfs" ] || [ "$FSTYPE" = "ext4" ] || [ "$FSTYPE" = "vfat" ] || [ "$FSTYPE" = "swap" ]; then
      continue
    fi
    kmsg_log "Prüfe NTFS-Partition $part..."

    ntfsfix -b -d "$part" >/dev/null 2>&1 || true

    if ntfs-3g -o ro "$part" /mnt/win_src 2>/dev/null || \
       ntfs-3g -o remove_hiberfile "$part" /mnt/win_src 2>/dev/null || \
       mount -t ntfs3 -o ro "$part" /mnt/win_src 2>/dev/null; then
      FOUND=$(find /mnt/win_src -iname "bifrost_blackbox.tar.zst" 2>/dev/null | head -n 1 || true)
      if [ -n "$FOUND" ]; then
        kmsg_log "Blackbox gefunden auf $part: $FOUND"
        kmsg_log "Kopiere Blackbox in den RAM ($ARCHIVE_RAM)..."
        cp "$FOUND" "$ARCHIVE_RAM"
        WIN_PART="$part"

        FOUND_ROOTFS=$(find /mnt/win_src -maxdepth 5 \( -iname "bifrost_rootfs.tar.zst" -o -iname "bifrost_rootfs.tar.gz" -o -iname "bifrost_rootfs.tar.xz" -o -iname "archlinux-bootstrap*.tar.zst" -o -iname "ubuntu-base*.tar.gz" \) 2>/dev/null | head -n 1 || true)
        if [ -n "$FOUND_ROOTFS" ]; then
          ROOTFS_DEST="$ROOTFS_RAM"
          case "$FOUND_ROOTFS" in
            *.tar.gz|*.tgz) ROOTFS_DEST="/tmp/bifrost_rootfs.tar.gz" ;;
            *.tar.xz|*.txz) ROOTFS_DEST="/tmp/bifrost_rootfs.tar.xz" ;;
          esac
          ROOTFS_KB=$(du -k "$FOUND_ROOTFS" 2>/dev/null | awk '{print $1}')
          TMP_AVAIL_KB=$(df -k /tmp 2>/dev/null | tail -n 1 | awk '{print $4}')
          if [ -n "$ROOTFS_KB" ] && [ -n "$TMP_AVAIL_KB" ] && [ "$TMP_AVAIL_KB" -gt "$((ROOTFS_KB + 32768))" ]; then
            kmsg_log "Staged Linux Rootfs gefunden: $FOUND_ROOTFS -> RAM ($ROOTFS_DEST)"
            cp "$FOUND_ROOTFS" "$ROOTFS_DEST" || rm -f "$ROOTFS_DEST"
          else
            kmsg_log "[WARN] Zu wenig RAM in /tmp (${TMP_AVAIL_KB:-0} KB) fuer Rootfs (${ROOTFS_KB:-0} KB) - nutze eingebettetes Asgard-Rootfs."
          fi
        fi

        umount /mnt/win_src || true
        break
      fi
      umount /mnt/win_src || true
    fi
  done

  # Fallback Mode B (Linux-to-Linux RAMFS Transit): Pruefe ext4/btrfs Transit-Quellen falls kein NTFS-Treffer vorlag
  if [ -z "$WIN_PART" ]; then
    for part in /dev/sd[a-z][0-9]* /dev/vd[a-z][0-9]* /dev/nvme[0-9]n[0-9]p[0-9]*; do
      [ -b "$part" ] || continue
      FSTYPE=$(blkid -s TYPE -o value "$part" 2>/dev/null || true)
      if [ "$FSTYPE" != "ext4" ] && [ "$FSTYPE" != "btrfs" ]; then
        continue
      fi
      if mount -t "$FSTYPE" -o ro "$part" /mnt/win_src 2>/dev/null; then
        if [ ! -f /mnt/win_src/.bifrost_installed ]; then
          FOUND=$(find /mnt/win_src -maxdepth 5 -path "*bifrost_linux_stage*" -iname "bifrost_blackbox.tar.zst" 2>/dev/null | head -n 1 || true)
          if [ -n "$FOUND" ]; then
            kmsg_log "Linux-Transit Blackbox gefunden auf $part: $FOUND"
            cp "$FOUND" "$ARCHIVE_RAM"
            WIN_PART="$part"
            FOUND_ROOTFS=$(find /mnt/win_src -maxdepth 5 -path "*bifrost_linux_stage*" \( -iname "bifrost_rootfs.tar.zst" -o -iname "bifrost_rootfs.tar.gz" -o -iname "bifrost_rootfs.tar.xz" \) 2>/dev/null | head -n 1 || true)
            if [ -n "$FOUND_ROOTFS" ]; then
              ROOTFS_DEST="$ROOTFS_RAM"
              case "$FOUND_ROOTFS" in
                *.tar.gz|*.tgz) ROOTFS_DEST="/tmp/bifrost_rootfs.tar.gz" ;;
                *.tar.xz|*.txz) ROOTFS_DEST="/tmp/bifrost_rootfs.tar.xz" ;;
              esac
              cp "$FOUND_ROOTFS" "$ROOTFS_DEST" 2>/dev/null || rm -f "$ROOTFS_DEST"
            fi
            umount /mnt/win_src || true
            break
          fi
        fi
        umount /mnt/win_src || true
      fi
    done
  fi

  # Boot #2 Check: Wenn keine offene Blackbox-Migration ansteht, pruefe auf bereits installiertes Linux-System (Btrfs oder Ext4)!
  if [ -z "$WIN_PART" ]; then
    for part in /dev/sd[a-z][0-9]* /dev/vd[a-z][0-9]* /dev/nvme[0-9]n[0-9]p[0-9]*; do
      [ -b "$part" ] || continue
      FSTYPE=$(blkid -s TYPE -o value "$part" 2>/dev/null || true)
      MOUNTED_ROOT=0
      if [ "$FSTYPE" = "btrfs" ]; then
        if mount -t btrfs -o noatime,compress=zstd:1,space_cache=v2,subvol=@root "$part" /mnt/target_linux 2>/dev/null; then
          MOUNTED_ROOT=1
        fi
      elif [ "$FSTYPE" = "ext4" ]; then
        if mount -t ext4 -o noatime "$part" /mnt/target_linux 2>/dev/null; then
          MOUNTED_ROOT=1
        fi
      fi

      if [ "$MOUNTED_ROOT" = "1" ]; then
        if [ -f /mnt/target_linux/.bifrost_installed ]; then
          kmsg_log "Installiertes Zielsystem ($FSTYPE) auf $part gefunden - starte Pivot/Boot!"
          mkdir -p /mnt/target_linux/home /mnt/target_linux/var/lib/bifrost /mnt/target_linux/.snapshots /mnt/target_linux/boot/efi /mnt/target_linux/proc /mnt/target_linux/sys /mnt/target_linux/dev /mnt/target_linux/run /mnt/target_linux/tmp
          if [ "$FSTYPE" = "btrfs" ]; then
            mount -t btrfs -o noatime,compress=zstd:1,space_cache=v2,subvol=@home "$part" /mnt/target_linux/home 2>/dev/null || true
            mount -t btrfs -o noatime,compress=zstd:1,space_cache=v2,subvol=@blackbox "$part" /mnt/target_linux/var/lib/bifrost 2>/dev/null || true
            mount -t btrfs -o noatime,compress=zstd:1,space_cache=v2,subvol=@snapshots "$part" /mnt/target_linux/.snapshots 2>/dev/null || true
          fi

          for efip in /dev/sd[a-z][0-9]* /dev/vd[a-z][0-9]* /dev/nvme[0-9]n[0-9]p[0-9]*; do
            [ -b "$efip" ] || continue
            if [ "$(blkid -s TYPE -o value "$efip" 2>/dev/null || true)" = "vfat" ]; then
              mount -t vfat "$efip" /mnt/target_linux/boot/efi 2>/dev/null || true
              break
            fi
          done

          # Auto-Netzwerk vor switch_root aktivieren (IPv4 & Routen bleiben im Kernel erhalten!)
          bring_up_network
          if [ -s /etc/resolv.conf ]; then
            cp /etc/resolv.conf /mnt/target_linux/etc/resolv.conf 2>/dev/null || true
          elif [ ! -s /mnt/target_linux/etc/resolv.conf ]; then
            echo "nameserver 1.1.1.1" > /mnt/target_linux/etc/resolv.conf 2>/dev/null || true
          fi

          install_target_runtime_helpers /mnt/target_linux

          mount --move /proc /mnt/target_linux/proc
          mount --move /sys /mnt/target_linux/sys
          mount --move /dev /mnt/target_linux/dev
          mount --move /run /mnt/target_linux/run 2>/dev/null || true
          mkdir -p /mnt/target_linux/dev/pts /mnt/target_linux/dev/shm
          mount -t devpts devpts /mnt/target_linux/dev/pts 2>/dev/null || true
          mount -t tmpfs tmpfs /mnt/target_linux/dev/shm 2>/dev/null || true

          clear 2>/dev/null || true
          if [ -x /mnt/target_linux/sbin/init ] || [ -L /mnt/target_linux/sbin/init ]; then
            exec switch_root /mnt/target_linux /sbin/init
          elif [ -x /mnt/target_linux/usr/local/sbin/bifrost-init ]; then
            exec switch_root /mnt/target_linux /usr/local/sbin/bifrost-init
          else
            exec chroot /mnt/target_linux /bin/sh -l </dev/tty0 >/dev/tty0 2>&1
          fi
        fi
        umount /mnt/target_linux 2>/dev/null || true
      fi
    done
  fi

  if [ -n "$WIN_PART" ] && [ -f "$ARCHIVE_RAM" ]; then
    # Mute console printk so kernel logs don't overwrite ratatui TUI
    echo "1 1 1 1" > /proc/sys/kernel/printk 2>/dev/null || true
    clear 2>/dev/null || true

    kmsg_log "Starte Bifröst RAMFS Transit Cinema Engine..."
    if /bin/bifroest --ramfs --archive "$ARCHIVE_RAM" --target-root /mnt/target_linux </dev/tty0 >/dev/tty0 2>&1; then
      echo "7 7 1 7" > /proc/sys/kernel/printk 2>/dev/null || true
      kmsg_log "=== Bifrost Migration erfolgreich abgeschlossen! ==="

      # Ensure kernel modules & runtime helpers are present inside installed root
      install_target_runtime_helpers /mnt/target_linux

      sync
      for mnt in /mnt/target_linux/boot/efi /mnt/target_linux/home /mnt/target_linux/var/lib/bifrost /mnt/target_linux/.snapshots /mnt/target_linux; do
        umount "$mnt" 2>/dev/null || true
      done

      # In Dual-Boot mode, rename source blackbox on NTFS so Boot #2 enters Asgard Linux directly
      if [ "$(blkid -s TYPE -o value "$WIN_PART" 2>/dev/null || true)" = "ntfs" ]; then
        if ntfs-3g "$WIN_PART" /mnt/win_src 2>/dev/null; then
          find /mnt/win_src -iname "bifrost_blackbox.tar.zst" 2>/dev/null | while read -r bbfile; do
            mv "$bbfile" "${bbfile%.tar.zst}.migrated.tar.zst" 2>/dev/null || true
          done
          umount /mnt/win_src 2>/dev/null || true
        fi
      fi

      sync
      kmsg_log "=== Migration abgeschlossen! Neustart nach Asgard Linux in 3 Sekunden... ==="
      sleep 3
      reboot -f 2>/dev/null || (echo b > /proc/sysrq-trigger 2>/dev/null) || sync
    else
      echo "7 7 1 7" > /proc/sys/kernel/printk 2>/dev/null || true
      kmsg_log "[ERROR] Bifröst RAMFS Engine meldete einen Fehler!"
    fi
  else
    echo "1 1 1 1" > /proc/sys/kernel/printk 2>/dev/null || true
    clear 2>/dev/null || true
    kmsg_log "[WARN] Starte interne Bifröst Auto-Discovery..."
    /bin/bifroest --ramfs --target-root /mnt/target_linux </dev/tty0 >/dev/tty0 2>&1 || true
    echo "7 7 1 7" > /proc/sys/kernel/printk 2>/dev/null || true
  fi
fi

kmsg_log "Bifröst RAMFS Durchlauf beendet. Automatischer Neustart..."
sleep 3
reboot -f 2>/dev/null || (echo b > /proc/sysrq-trigger 2>/dev/null) || sync

# Safety loop: PID 1 must NEVER exit and NEVER panic!
while true; do
  reboot -f 2>/dev/null || (echo b > /proc/sysrq-trigger 2>/dev/null) || sleep 60
done
EOF

chmod +x "$WORK_DIR/init" "$WORK_DIR/bin/bifroest"

echo "[Bifrost] Packe CPIO + Gzip Archiv nach $OUT_FILE..."
(cd "$WORK_DIR" && find . -print0 | cpio --null -ov --format=newc | gzip -9) > "$OUT_FILE"
echo "[OK] Initramfs erstellt: $OUT_FILE ($(du -h "$OUT_FILE" | cut -f1))"
