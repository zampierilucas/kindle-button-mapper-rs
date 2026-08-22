#!/bin/sh
set -e

trap '/usr/sbin/mntroot ro 2>/dev/null' EXIT
trap 'exit 130' INT TERM HUP

INSTALL_DIR=/mnt/us/kindle-button-mapper
APPREG_DB=/var/local/appreg.db
APP_ID=com.lzampier.mappermanager

/sbin/initctl stop kindle-button-mapper 2>/dev/null || true

# Helper + WAF mesquite — release any handles still open on the install dir.
lipc-set-prop com.lab126.appmgrd start app://com.lab126.booklet.home 2>/dev/null || true
pkill -TERM -f mesquite.*mappermanager 2>/dev/null || true
kill "$(cat /tmp/kindle-button-mapper-waf.pid 2>/dev/null)" 2>/dev/null || true
pkill -f waf-helper 2>/dev/null || true
sleep 1

/usr/sbin/mntroot rw

rm -f /etc/upstart/kindle-button-mapper.conf
rm -f /etc/udev/rules.d/99-kindle-button-mapper-pointer.rules
# Drop the job from upstart now that its .conf is gone, so it isn't left as a
# known-but-fileless job until the next reboot.
/sbin/initctl reload-configuration 2>/dev/null || true

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

echo "Uninstalled."
