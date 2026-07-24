#!/bin/sh
# install-waf-app.sh - Install MapperManager WAF app (Illusion)
# Run on the Kindle to register and set up the Button Mapper WAF app.
# Also handles UPstart configuration for kindle-button-mapper.

APP_ID="com.lzampier.mappermanager"
APP_DIR="/mnt/us/kindle-button-mapper/illusion/MapperManager"
ILLUSION_DIR="/mnt/us/kindle-button-mapper/illusion"
BINARY="/mnt/us/kindle-button-mapper/kindle-button-mapper"
SCRIPTLET="$ILLUSION_DIR/MapperManager.sh"
SCRIPTLET_DEST="/mnt/us/documents/MapperManager.sh"
APPREG_DB="/var/local/appreg.db"
UPSTART_SRC="/mnt/us/kindle-button-mapper/assets/kindle-button-mapper.upstart"
UPSTART_DEST="/etc/upstart/kindle-button-mapper.conf"

echo ""
echo "=== MapperManager Installer ==="
echo ""

if [ ! -f "$APP_DIR/config.xml" ]; then
    echo "ERROR: App files not found at $APP_DIR"
    echo "Deploy kindle-button-mapper first (just deploy)."
    exit 1
fi

if [ ! -x "$BINARY" ]; then
    echo "ERROR: kindle-button-mapper binary not found at $BINARY"
    exit 1
fi

# -------------------------------------------------------------
# Interactive Menu: Choose installation options
# -------------------------------------------------------------
echo "Select installation option:"
echo "  1) Install ALL components (App registration + Scriptlet + UPstart system service)"
echo "  2) Install ALL components EXCEPT UPstart (Without modifying system partition)"
echo ""
printf "Enter option [1 or 2]: "
read CHOICE

case "$CHOICE" in
    1)
        INSTALL_UPSTART=1
        echo "\n--> Selected: Install all components"
        ;;
    2)
        INSTALL_UPSTART=0
        echo "\n--> Selected: Skip UPstart service installation"
        ;;
    *)
        echo "\nInvalid selection. Installation aborted."
        exit 1
        ;;
esac

echo ""
echo "1. App files at $APP_DIR"

echo "2. Setting scriptlet permissions"
chmod +x "$SCRIPTLET" 2>/dev/null

echo "3. Registering app"
if [ -f "$APPREG_DB" ]; then
    existing=$(sqlite3 "$APPREG_DB" "SELECT handlerId FROM handlerIds WHERE handlerId='$APP_ID';" 2>/dev/null)
    if [ -z "$existing" ]; then
        sqlite3 "$APPREG_DB" <<EOF
INSERT OR IGNORE INTO interfaces (interface) VALUES ('application');
INSERT OR IGNORE INTO handlerIds (handlerId) VALUES ('$APP_ID');
INSERT OR IGNORE INTO associations (handlerId, interface, contentId, defaultAssoc)
    VALUES ('$APP_ID', 'application', 'GL:$APP_ID', 0);
INSERT OR REPLACE INTO properties (handlerId, name, value)
    VALUES ('$APP_ID', 'lipcId', '$APP_ID');
INSERT OR REPLACE INTO properties (handlerId, name, value)
    VALUES ('$APP_ID', 'command', '/usr/bin/mesquite -l $APP_ID -c file://$APP_DIR/');
INSERT OR REPLACE INTO properties (handlerId, name, value)
    VALUES ('$APP_ID', 'supportedOrientation', 'U');
EOF
        echo "   Registered $APP_ID"
    else
        echo "   Already registered"
    fi
else
    echo "   WARNING: appreg.db not found at $APPREG_DB"
fi

echo "4. Installing scriptlet"
cp "$SCRIPTLET" "$SCRIPTLET_DEST"
chmod +x "$SCRIPTLET_DEST"
echo "   Installed at $SCRIPTLET_DEST"

# -------------------------------------------------------------
# 5. UPstart configuration installation (conditional on CHOICE)
# -------------------------------------------------------------
if [ "$INSTALL_UPSTART" -eq 1 ]; then
    echo "5. Installing UPstart configuration"
    /usr/sbin/mntroot rw 2>/dev/null

    if [ -f "$UPSTART_SRC" ]; then
        cp "$UPSTART_SRC" "$UPSTART_DEST"
        echo "   Copied UPstart config to $UPSTART_DEST"
        /sbin/initctl reload-configuration 2>/dev/null || true
        if /sbin/initctl status kindle-button-mapper 2>/dev/null | grep -q "start/running"; then
            /sbin/initctl restart kindle-button-mapper 2>/dev/null || true
        else
            /sbin/initctl start kindle-button-mapper 2>/dev/null || true
        fi
    else
        echo "   WARNING: UPstart source file not found at $UPSTART_SRC"
    fi

    /usr/sbin/mntroot ro 2>/dev/null || true
else
    echo "5. Skipping UPstart configuration (Option 2 selected)"
fi

echo ""
echo "=== Installation Complete ==="
echo ""
echo "Open 'Button Mapper' (MapperManager.sh) from the Kindle library, or run:"
echo "  lipc-set-prop com.lab126.appmgrd start app://$APP_ID"
echo ""
