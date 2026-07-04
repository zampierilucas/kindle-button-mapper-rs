#!/bin/sh

set -e

INSTALL_DIR=/mnt/us/kindle-button-mapper
APPREG_DB=/var/local/appreg.db
APP_ID="com.lzampier.mappermanager"

/sbin/initctl stop kindle-button-mapper 2>/dev/null || true

/usr/sbin/mntroot rw

rm -f /etc/upstart/kindle-button-mapper.conf

if [ -f "$APPREG_DB" ]; then
    sqlite3 "$APPREG_DB" <<EOF
DELETE FROM properties WHERE handlerId='$APP_ID';
DELETE FROM associations WHERE handlerId='$APP_ID';
DELETE FROM handlerIds WHERE handlerId='$APP_ID';
EOF
fi

/usr/sbin/mntroot ro || true

rm -rf "$INSTALL_DIR"
rm -f /mnt/us/documents/MapperManager.sh

exit 0
