#!/bin/sh
set -e

VMLINUZ="${1:-vmlinuz-lts}"
INITRD="${2:-initramfs-bifrost.cpio.gz}"
OUT_EFI="${3:-bifrost-boot.efi}"
CMDLINE="bifrost_auto=1 console=tty0 earlycon=efifb nomodeset loglevel=7"

if [ ! -f "$VMLINUZ" ]; then
  if [ -f /boot/vmlinuz-lts ]; then
    VMLINUZ="/boot/vmlinuz-lts"
  elif [ -f /boot/vmlinuz-virt ]; then
    VMLINUZ="/boot/vmlinuz-virt"
  else
    echo "[ERROR] Kernel nicht gefunden: $VMLINUZ"
    exit 1
  fi
fi

if [ ! -f "$INITRD" ]; then
  echo "[Bifrost] Initramfs nicht gefunden ($INITRD) - baue via tools/make_initramfs.sh..."
  sh "$(dirname "$0")/make_initramfs.sh" "$INITRD"
fi

STUB=""
for candidate in \
  /usr/lib/systemd/boot/efi/linuxx64.efi.stub \
  /usr/libexec/systemd/boot/efi/linuxx64.efi.stub \
  /usr/share/systemd/boot/efi/linuxx64.efi.stub; do
  if [ -f "$candidate" ]; then
    STUB="$candidate"
    break
  fi
done

if [ -z "$STUB" ]; then
  echo "[ERROR] linuxx64.efi.stub nicht gefunden (systemd-boot-efi installieren)!"
  exit 1
fi

WORK_DIR="$(mktemp -d /tmp/bifrost_uki.XXXXXX)"
trap 'rm -rf "$WORK_DIR"' EXIT

cat << 'EOF' > "$WORK_DIR/os-release"
NAME="Bifrost RAMFS"
PRETTY_NAME="Bifrost Linux Bridge (RAMFS)"
ID=bifrost
VERSION_ID="0.1.0"
EOF

printf '%s' "$CMDLINE" > "$WORK_DIR/cmdline.txt"

UKIFY_BIN="$(command -v ukify || true)"
if [ -z "$UKIFY_BIN" ] && [ -x /usr/lib/systemd/ukify ]; then
  UKIFY_BIN="/usr/lib/systemd/ukify"
fi

if [ -n "$UKIFY_BIN" ]; then
  echo "[Bifrost] Baue UKI via $UKIFY_BIN..."
  "$UKIFY_BIN" build \
    --linux="$VMLINUZ" \
    --initrd="$INITRD" \
    --cmdline="$CMDLINE" \
    --os-release="@$WORK_DIR/os-release" \
    --stub="$STUB" \
    --output="$OUT_EFI"
else
  echo "[Bifrost] Baue UKI via objcopy Fallback (PE Section Alignment)..."
  ALIGN="$(objdump -p "$STUB" | awk '/SectionAlignment/ {print strtonum("0x"$2)}')"
  [ -z "$ALIGN" ] && ALIGN=4096
  LAST_END="$(objdump -h "$STUB" | awk 'NF>=6 && /^[ ]*[0-9]+/ { size=strtonum("0x"$3); vma=strtonum("0x"$4); end=vma+size } END { print end }')"
  align_up() {
    echo $(( (($1 + ALIGN - 1) / ALIGN) * ALIGN ))
  }
  IMAGE_BASE="$(objdump -p "$STUB" | awk '/ImageBase/ {print strtonum("0x"$2)}')"
  [ -z "$IMAGE_BASE" ] && IMAGE_BASE=0

  OSREL_OFF=$(align_up "$LAST_END")
  OSREL_SIZE=$(wc -c < "$WORK_DIR/os-release")
  CMD_OFF=$(align_up $((OSREL_OFF + OSREL_SIZE)))
  CMD_SIZE=$(wc -c < "$WORK_DIR/cmdline.txt")
  LINUX_OFF=$(align_up $((CMD_OFF + CMD_SIZE)))
  LINUX_SIZE=$(wc -c < "$VMLINUZ")
  INITRD_OFF=$(align_up $((LINUX_OFF + LINUX_SIZE)))

  objcopy \
    --add-section .osrel="$WORK_DIR/os-release" --change-section-vma .osrel=$(printf '0x%x' $((IMAGE_BASE + OSREL_OFF))) \
    --add-section .cmdline="$WORK_DIR/cmdline.txt" --change-section-vma .cmdline=$(printf '0x%x' $((IMAGE_BASE + CMD_OFF))) \
    --add-section .linux="$VMLINUZ" --change-section-vma .linux=$(printf '0x%x' $((IMAGE_BASE + LINUX_OFF))) \
    --add-section .initrd="$INITRD" --change-section-vma .initrd=$(printf '0x%x' $((IMAGE_BASE + INITRD_OFF))) \
    "$STUB" "$OUT_EFI"
fi

if [ ! -f "$OUT_EFI" ]; then
  echo "[ERROR] UKI konnte nicht erstellt werden: $OUT_EFI"
  exit 1
fi

SIZE_BYTES=$(wc -c < "$OUT_EFI")
if [ "$SIZE_BYTES" -le 5242880 ]; then
  echo "[ERROR] UKI ist verdächtig klein ($SIZE_BYTES Bytes <= 5 MB)!"
  exit 1
fi

echo "[OK] Unified Kernel Image bereit: $OUT_EFI ($SIZE_BYTES Bytes)"
