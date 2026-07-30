#!/bin/sh
# Install kindle-button-mapper. Works from two layouts:
#   1. A source checkout (binary under target/armv7-.../release/).
#   2. An extracted release tarball (binary sitting at the top level).
# Run it on the Kindle after copying the tree there, or via:
#   ssh kindle "sh /mnt/us/kindle-button-mapper-src/install.sh"

set -e

SRC_DIR=$(cd "$(dirname "$0")" && pwd)
INSTALL_DIR=/mnt/us/kindle-button-mapper

if [ -x "$SRC_DIR/kindle-button-mapper" ]; then
    BIN="$SRC_DIR/kindle-button-mapper"
elif [ -x "$SRC_DIR/target/armv7-unknown-linux-musleabihf/release/kindle-button-mapper" ]; then
    BIN="$SRC_DIR/target/armv7-unknown-linux-musleabihf/release/kindle-button-mapper"
else
    echo "ERROR: no kindle-button-mapper binary found." >&2
    echo "Extract a release tarball here, or build from source:" >&2
    echo "  cargo build --release --target armv7-unknown-linux-musleabihf" >&2
    exit 1
fi

mkdir -p "$INSTALL_DIR/scripts" "$INSTALL_DIR/illusion/MapperManager" "$INSTALL_DIR/assets"

# The tree may already have been copied straight into INSTALL_DIR; only copy
# what isn't already in place (busybox cp aborts on same-file copies).
# Probe with a sentinel instead of comparing path strings, since /mnt/us is
# case-insensitive vfat and also reachable via the /mnt/base-us alias mount.
SAME_DIR=0
SENTINEL=".kbm-install-probe.$$"
if : > "$INSTALL_DIR/$SENTINEL" 2>/dev/null; then
    [ -e "$SRC_DIR/$SENTINEL" ] && SAME_DIR=1
    rm -f "$INSTALL_DIR/$SENTINEL"
fi

if [ "$SAME_DIR" = 1 ] && [ "$BIN" = "$SRC_DIR/kindle-button-mapper" ]; then
    : # binary already sits at its destination
else
    cp "$BIN" "$INSTALL_DIR/kindle-button-mapper"
fi
if [ "$SAME_DIR" != 1 ]; then
    cp "$SRC_DIR/assets/kindle-button-mapper.upstart" "$INSTALL_DIR/assets/"
    cp "$SRC_DIR/uninstall.sh" "$INSTALL_DIR/"
    [ -f "$INSTALL_DIR/config.ini" ] || cp "$SRC_DIR/config.ini" "$INSTALL_DIR/"
    cp "$SRC_DIR/scripts/"*.sh "$INSTALL_DIR/scripts/"
    cp "$SRC_DIR/illusion/MapperManager.sh" "$SRC_DIR/illusion/install-waf-app.sh" "$INSTALL_DIR/illusion/"
    cp "$SRC_DIR/illusion/MapperManager/"* "$INSTALL_DIR/illusion/MapperManager/"
fi
rm -f "$INSTALL_DIR/scripts/start-inhib.sh" "$INSTALL_DIR/scripts/stop-inhib.sh"

chmod +x "$INSTALL_DIR/kindle-button-mapper" \
         "$INSTALL_DIR/uninstall.sh" \
         "$INSTALL_DIR/scripts/"*.sh \
         "$INSTALL_DIR/illusion/"*.sh

/usr/sbin/mntroot rw

cp "$INSTALL_DIR/assets/kindle-button-mapper.upstart" /etc/upstart/kindle-button-mapper.conf

cp "$INSTALL_DIR/illusion/MapperManager.sh" /mnt/us/documents/MapperManager.sh
chmod +x /mnt/us/documents/MapperManager.sh

sh "$INSTALL_DIR/illusion/install-waf-app.sh" || true

/usr/sbin/mntroot ro || true

# upstart only picks up a newly dropped .conf after a config reload; without
# this the job stays "Unknown" (so the start below fails) until a reboot.
/sbin/initctl reload-configuration || true
/sbin/initctl restart kindle-button-mapper || /sbin/initctl start kindle-button-mapper

echo "Installed. Open Button Mapper from the Kindle library or via:"
echo "  lipc-set-prop com.lab126.appmgrd start app://com.lzampier.mappermanager"
