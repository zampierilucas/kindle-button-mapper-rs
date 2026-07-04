#!/bin/sh
# Install kindle-button-mapper from a local source checkout.
# Run this on the Kindle after copying the repo there, or use it via:
#   ssh kindle "sh /mnt/us/kindle-button-mapper-src/install.sh"

set -e

SRC_DIR=$(cd "$(dirname "$0")" && pwd)
INSTALL_DIR=/mnt/us/kindle-button-mapper

BIN="$SRC_DIR/target/armv7-unknown-linux-musleabihf/release/kindle-button-mapper"
if [ ! -x "$BIN" ]; then
    echo "ERROR: build the binary first:" >&2
    echo "  cargo build --release --target armv7-unknown-linux-musleabihf" >&2
    exit 1
fi

mkdir -p "$INSTALL_DIR/scripts" "$INSTALL_DIR/illusion/MapperManager" "$INSTALL_DIR/assets"

cp "$BIN" "$INSTALL_DIR/kindle-button-mapper"
cp "$SRC_DIR/assets/kindle-button-mapper.upstart" "$INSTALL_DIR/assets/"
cp "$SRC_DIR/uninstall.sh" "$INSTALL_DIR/"
[ -f "$INSTALL_DIR/config.ini" ] || cp "$SRC_DIR/config.ini" "$INSTALL_DIR/"
cp "$SRC_DIR/scripts/"*.sh "$INSTALL_DIR/scripts/"
rm -f "$INSTALL_DIR/scripts/start-inhib.sh" "$INSTALL_DIR/scripts/stop-inhib.sh"
cp "$SRC_DIR/illusion/MapperManager.sh" "$SRC_DIR/illusion/install-waf-app.sh" "$INSTALL_DIR/illusion/"
cp "$SRC_DIR/illusion/MapperManager/"* "$INSTALL_DIR/illusion/MapperManager/"

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

/sbin/initctl restart kindle-button-mapper || /sbin/initctl start kindle-button-mapper

echo "Installed. Open Button Mapper from the Kindle library or via:"
echo "  lipc-set-prop com.lab126.appmgrd start app://com.lzampier.mappermanager"
