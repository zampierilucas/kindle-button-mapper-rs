#!/bin/sh

set -e

trap '/usr/sbin/mntroot ro 2>/dev/null' EXIT
trap 'exit 130' INT TERM HUP

TMPDIR=/mnt/us/KFPM-Temporary
INSTALL_DIR=/mnt/us/kindle-button-mapper
RELEASE_URL="https://github.com/zampierilucas/kindle-button-mapper-rs/releases/latest/download"

mkdir -p "$TMPDIR" "$INSTALL_DIR"

curl -fSL --progress-bar -o "$TMPDIR/release.tar.gz" \
    "$RELEASE_URL/kindle-button-mapper-armv7.tar.gz"
tar -xzf "$TMPDIR/release.tar.gz" -C "$INSTALL_DIR"

chmod +x "$INSTALL_DIR/kindle-button-mapper"
chmod +x "$INSTALL_DIR/scripts/"*.sh 2>/dev/null || true
chmod +x "$INSTALL_DIR/illusion/"*.sh 2>/dev/null || true

/usr/sbin/mntroot rw

cp "$INSTALL_DIR/assets/kindle-button-mapper.upstart" /etc/upstart/kindle-button-mapper.conf

cp "$INSTALL_DIR/illusion/MapperManager.sh" /mnt/us/documents/MapperManager.sh
chmod +x /mnt/us/documents/MapperManager.sh

sh "$INSTALL_DIR/illusion/install-waf-app.sh" || true

/usr/sbin/mntroot ro || true

# upstart only picks up a newly dropped .conf after a config reload; without
# this the job stays "Unknown" (so the start below fails) until a reboot.
/sbin/initctl reload-configuration || true
/sbin/initctl start kindle-button-mapper || true

rm -rf "$TMPDIR"

exit 0
