# Kindle Button Mapper (Rust) - Build and Deploy

# Default recipe
default:
    @just --list

# Build for local testing
build:
    cargo build

# Build release for local testing
build-release:
    cargo build --release

# Build for Kindle (ARM, static musl - no glibc dependency)
build-kindle:
    cargo build --release --target armv7-unknown-linux-musleabihf

# Run locally (requires root for /dev/input access)
run:
    sudo RUST_LOG=debug cargo run

# Run with a specific config file
run-config config:
    sudo RUST_LOG=debug cargo run -- {{config}}

# Check code
check:
    cargo check
    cargo clippy

# Format code
fmt:
    cargo fmt

# Run tests
test:
    cargo test

# Clean build artifacts
clean:
    cargo clean

# Deploy to Kindle
deploy: build-kindle
    @echo "Deploying to Kindle..."
    @echo "Stopping daemon..."
    -ssh kindle "/etc/init.d/kindle-button-mapper stop" 2>/dev/null || true
    @echo "Remounting filesystems as writable..."
    ssh kindle "/usr/sbin/mntroot rw && mount -o remount,rw /mnt/base-us"
    @echo "Creating directories..."
    ssh kindle "mkdir -p /mnt/us/kindle-button-mapper/scripts"
    @echo "Copying files..."
    # scp can't overwrite the running binary (busy on vfat); copy to a temp name and rename.
    scp target/armv7-unknown-linux-musleabihf/release/kindle-button-mapper kindle:/mnt/us/kindle-button-mapper/kindle-button-mapper.new
    ssh kindle "mv -f /mnt/us/kindle-button-mapper/kindle-button-mapper.new /mnt/us/kindle-button-mapper/kindle-button-mapper && chmod +x /mnt/us/kindle-button-mapper/kindle-button-mapper"
    @echo "Copying config (only if absent, to preserve device bindings)..."
    ssh kindle "test -f /mnt/us/kindle-button-mapper/config.ini" || scp config.ini kindle:/mnt/us/kindle-button-mapper/
    scp scripts/*.sh kindle:/mnt/us/kindle-button-mapper/scripts/
    ssh kindle "chmod +x /mnt/us/kindle-button-mapper/scripts/*.sh"
    scp kindle-button-mapper.init kindle:/etc/init.d/kindle-button-mapper
    ssh kindle "chmod +x /etc/init.d/kindle-button-mapper"
    @echo "Deployment complete!"
    @echo ""
    @echo "Start daemon with: just start"
    @echo "View logs with: just logs"

# Enable autostart on boot (adds to /etc/rc.local)
enable:
    ssh kindle "grep -q 'kindle-button-mapper' /etc/rc.local || sed -i '/^exit 0/i /etc/init.d/kindle-button-mapper start \&' /etc/rc.local"
    @echo "Autostart enabled!"

# Disable autostart on boot (removes from /etc/rc.local)
disable:
    ssh kindle "sed -i '/kindle-button-mapper/d' /etc/rc.local"
    @echo "Autostart disabled!"

# Start daemon on Kindle
start:
    ssh kindle "/etc/init.d/kindle-button-mapper start"

# Stop daemon on Kindle
stop:
    ssh kindle "/etc/init.d/kindle-button-mapper stop"

# Restart daemon on Kindle
restart:
    ssh kindle "/etc/init.d/kindle-button-mapper restart"

# Check daemon status
status:
    ssh kindle "/etc/init.d/kindle-button-mapper status"

# Follow daemon logs
logs:
    ssh kindle "tail -f /var/log/kindle-button-mapper.log"

# Show recent logs
logs-recent:
    ssh kindle "tail -50 /var/log/kindle-button-mapper.log"

# Deploy, restart, and follow logs
deploy-watch: deploy restart logs

# Deploy WAF + helper, gracefully relaunch with fresh JS, no popup
deploy-waf:
    -ssh kindle "lipc-set-prop com.lab126.appmgrd start app://com.lab126.booklet.home" 2>/dev/null
    -ssh kindle "kill \$(cat /tmp/kindle-button-mapper-waf.pid 2>/dev/null) 2>/dev/null"
    ssh kindle "/usr/sbin/mntroot rw && mkdir -p /mnt/us/kindle-button-mapper/illusion/MapperManager"
    scp illusion/MapperManager/config.xml illusion/MapperManager/index.html illusion/MapperManager/style.css illusion/MapperManager/script.js kindle:/mnt/us/kindle-button-mapper/illusion/MapperManager/
    scp illusion/MapperManager.sh illusion/install-waf-app.sh kindle:/mnt/us/kindle-button-mapper/illusion/
    ssh kindle "chmod +x /mnt/us/kindle-button-mapper/illusion/*.sh"
    -ssh kindle "rm -rf /var/local/mesquite/com.lzampier.mappermanager" 2>/dev/null
    ssh kindle "nohup /mnt/us/kindle-button-mapper/kindle-button-mapper --waf-helper /mnt/us/kindle-button-mapper/config.ini </dev/null >/var/log/kindle-button-mapper-waf.log 2>&1 & echo \$! > /tmp/kindle-button-mapper-waf.pid; disown 2>/dev/null; sleep 1"
    -ssh kindle "pkill -TERM -f mesquite.*mappermanager" 2>/dev/null
    sleep 2
    ssh kindle "lipc-set-prop com.lab126.appmgrd start app://com.lzampier.mappermanager"
    @echo "First-time install: ssh kindle 'sh /mnt/us/kindle-button-mapper/illusion/install-waf-app.sh'"

# Tail the WAF helper log
logs-waf:
    ssh kindle "tail -f /var/log/kindle-button-mapper-waf.log"

# List input devices on Kindle
list-devices:
    ssh kindle "cat /proc/bus/input/devices"

# Monitor raw events from a device
monitor-events device:
    ssh kindle "cat /dev/input/{{device}} | hexdump"

# Show binary size
size: build-release
    ls -lh target/release/kindle-button-mapper
    @echo "Stripped size:"
    strip -s target/release/kindle-button-mapper -o /tmp/stripped
    ls -lh /tmp/stripped

# Show Kindle binary size
size-kindle: build-kindle
    ls -lh target/armv7-unknown-linux-musleabihf/release/kindle-button-mapper

# Install cross (for ARM compilation)
install-cross:
    cargo install cross --git https://github.com/cross-rs/cross

# Line count
loc:
    @echo "Source lines:"
    @wc -l src/*.rs | tail -1
    @echo ""
    @tokei src/ || true
