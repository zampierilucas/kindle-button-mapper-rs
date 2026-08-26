#!/bin/sh
# install-waf-app.sh - Install MapperManager WAF app (Illusion)
# Run on the Kindle to register and set up the Button Mapper WAF app.

APP_ID="com.lzampier.mappermanager"
APP_DIR="/mnt/us/kindle-button-mapper/illusion/MapperManager"
ILLUSION_DIR="/mnt/us/kindle-button-mapper/illusion"
BINARY="/mnt/us/kindle-button-mapper/kindle-button-mapper"
SCRIPTLET="$ILLUSION_DIR/MapperManager.sh"
SCRIPTLET_DEST="/mnt/us/documents/MapperManager.sh"
APPREG_DB="/var/local/appreg.db"

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

echo "1. App files at $APP_DIR"

echo "2. Clearing cached app assets"
rm -rf "/var/local/mesquite/$APP_ID" /var/local/mesquite/MapperManager 2>/dev/null

echo "3. Setting scriptlet permissions"
chmod +x "$SCRIPTLET" 2>/dev/null

echo "4. Registering app"
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

echo "5. Installing scriptlet"
cp "$SCRIPTLET" "$SCRIPTLET_DEST"
chmod +x "$SCRIPTLET_DEST"
echo "   Installed at $SCRIPTLET_DEST"

echo ""
echo "=== Installation Complete ==="
echo ""
echo "Open 'Button Mapper' (MapperManager.sh) from the Kindle library, or run:"
echo "  lipc-set-prop com.lab126.appmgrd start app://$APP_ID"
echo ""
