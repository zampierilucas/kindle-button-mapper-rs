# Kindle Button Mapper

A Rust-based Linux input device event mapper for Kindle e-readers. Maps button presses from input devices to shell scripts.

## Features

- Map buttons to shell scripts
- Long press support with separate actions
- Auto-repeat when buttons are held
- Debouncing to prevent double-triggers
- Auto-reconnect on device disconnect
- Optional exclusive device grab
- Per-keyboard XKB layout, re-applied on every reconnect

## Building

```bash
cargo build --release
```

### Cross-Compilation (ARM)

```bash
cargo build --release --target armv7-unknown-linux-gnueabihf
```

## Usage

```bash
kindle-button-mapper /path/to/config.ini
```

Enable debug logging with `RUST_LOG=debug`.

## Configuration

INI format configuration file. Each device is its own `[device.ID]` block with
`[device.ID.{kind}]` subsections for the mappings:

```ini
[settings]
debounce_ms = 200
long_press_ms = 500
repeat_ms = 100
log_buttons = true
keep_awake = true
on_connect = /path/to/script.sh
on_disconnect = /path/to/script.sh

[device.gamepad]
name = Device Name
uniq = AA:BB:CC:DD:EE:FF   # Bluetooth MAC; matched first when set
grab = true
# keyboard_layout = fr     # XKB layout re-applied whenever this keyboard connects

[device.gamepad.buttons]
# button_code = /path/to/script.sh

[device.gamepad.longpress]
# button_code = /path/to/script.sh

[device.gamepad.dpad]
# up/down/left/right = /path/to/script.sh

[device.gamepad.dpad_longpress]
# up/down/left/right = /path/to/script.sh

[device.gamepad.triggers]
# lt/rt = /path/to/script.sh

[device.gamepad.triggers_longpress]
# lt/rt = /path/to/script.sh
```

Devices are matched by identity, never by `/dev/input/eventX` path (that index is
unstable across reconnects): the mapper uses the Bluetooth MAC (`uniq`) when set,
otherwise the device `name`. Set at least one.

Set `keyboard_layout` to an XKB layout code (e.g. `fr`, `de`, `ro`, `fr(oss)`) to remap a Bluetooth keyboard. The mapper re-applies it on every connect, so the layout survives reconnects instead of reverting to US. Leave it unset to keep the system default.

Use `log_buttons = true` to discover button codes for your device.

`keep_awake = true` (default) resets the screensaver timer on input so the device stays awake while a controller is connected, without blocking the power button.

## On-device UI (MapperManager WAF)

| Bindings | Device | Debug | Action picker |
|---|---|---|---|
| ![Bindings](docs/screenshots/bindings.png) | ![Device](docs/screenshots/device.png) | ![Debug](docs/screenshots/debug.png) | ![Action picker](docs/screenshots/action-picker.png) |

A touchscreen UI for editing mappings without SSH lives in `illusion/MapperManager/`. The daemon stays a plain runtime — the WAF app spawns a `--waf-helper` HTTP server (localhost:8322) only while the app is open, edits `config.ini`, and restarts the daemon via `initctl restart kindle-button-mapper`.

Deploy and register:

```bash
just deploy        # ship the binary + config + init script
just deploy-waf    # ship the illusion/ app, restart helper, launch WAF
ssh kindle "sh /mnt/us/kindle-button-mapper/illusion/install-waf-app.sh"   # first time only
```

The app has three tabs:
- **Bindings** — list of current button / D-pad / trigger mappings per device. Tap *+ Add* to capture a button and pick an action. Each binding can map to a KOReader command, a keyboard key, or a custom shell command.
- **Device** — list of configured devices, each matched by its Bluetooth MAC or name. Add, edit, or remove a device, or tap one seen on `/dev/input` to prefill its name and MAC.
- **Debug** — live button capture for discovering codes, and a raw `config.ini` editor.

## Install from release

Grab the latest `kindle-button-mapper-armv7.tar.gz` from the [releases page](https://github.com/zampierilucas/kindle-button-mapper-rs/releases), copy it to the Kindle, extract, and run the installer:

```bash
scp kindle-button-mapper-armv7.tar.gz kindle:/mnt/us/
ssh kindle "mkdir -p /mnt/us/kbm && tar -xzf /mnt/us/kindle-button-mapper-armv7.tar.gz -C /mnt/us/kbm && sh /mnt/us/kbm/install.sh"
```

The tarball ships the pre-built ARMv7 binary, so no host toolchain or cross-compile is needed. `install.sh` detects the extracted-tarball layout automatically.

## Install from source

```bash
# 1. Cross-compile the ARM binary on your host
rustup target add armv7-unknown-linux-musleabihf
cargo build --release --target armv7-unknown-linux-musleabihf

# 2. Copy the repo to the Kindle and run the installer
rsync -av --exclude target/ . kindle:/mnt/us/kindle-button-mapper-src/
ssh kindle "sh /mnt/us/kindle-button-mapper-src/install.sh"
```

Uninstall: `ssh kindle "sh /mnt/us/kindle-button-mapper/uninstall.sh"` (the script is copied to the install dir, so you can run it even after the source tree is gone).

## Requirements

- Jailbroken Kindle (Kindle 5+ / FW 5.x).
- Linux kernel with evdev (`/dev/input/eventX`) — present on all stock Kindles.
- An input device the Kindle can see — e.g. a Bluetooth gamepad/remote bridged via [kindle-hid-passthrough](https://github.com/zampierilucas/kindle-hid-passthrough), or any USB OTG HID device.
- **KOReader HTTP Inspector** (for KOReader integration): enable auto-start once in KOReader → *Tools → More Tools → HTTP Inspector → Auto-start HTTP server*. The default mappings in `scripts/koreader.sh` send commands to `localhost:8080`. MapperManager warns you in the KOReader action tab when this auto-start is off.

## Hardware

Tested on:
- **Device**: Kindle MT8110 Bellatrix (Paperwhite 12)
- **SoC**: MediaTek MT8512 (ARMv7-A Cortex-A53)
- **Kernel**: Linux 4.9.77-lab126

The release binary is a static ARMv7 musl build (~1.1 MB, no glibc dependency) and should work on any ARMv7 Kindle running a jailbroken FW that allows running native binaries from `/mnt/us`. No per-FW binary is required.

## Support

[![ko-fi](https://ko-fi.com/img/githubbutton_sm.svg)](https://ko-fi.com/lzampier)

## License

GPL-3.0-or-later — see [LICENSE](LICENSE).
