#!/bin/sh
# Tap the touchscreen.
# Usage: tap.sh [X_PERCENT] [Y_PERCENT]
#
# Writes the input_event structs a real finger produces straight into the
# touchscreen node, so it needs nothing but a shell. Codes the panel does not
# support are dropped by the kernel, which is why the sequence can carry both
# BTN_TOUCH and the pressure/contact axes and still suit either panel.
#
# Percentages are screen space, rotated into panel space before writing.
# TAP_ROTATION forces the angle, TAP_DRY_RUN prints the pixel and taps nothing.

X_PCT="${1:-85}"
Y_PCT="${2:-50}"

FB_MODES="/sys/class/graphics/fb0/modes"
DEVICES="/proc/bus/input/devices"

# The touchscreen is the direct-input device with absolute axes. A gamepad has
# axes too, but reports no INPUT_PROP_DIRECT.
touch_node() {
    awk '
        /^$/ { prop=""; abs=""; handler="" }
        /^B: PROP=/ { prop=$2; sub(/PROP=/, "", prop) }
        /^B: ABS=/  { abs=$2 }
        /^H: Handlers=/ {
            handler=""
            for (i = 2; i <= NF; i++) {
                t=$i; sub(/^Handlers=/, "", t)
                if (t ~ /^event[0-9]+$/) handler=t
            }
        }
        {
            if (handler != "" && abs != "" && prop ~ /[23671abefABEF]$/) {
                print "/dev/input/" handler
                exit
            }
        }
    ' "$DEVICES" 2>/dev/null
}

screen_size() {
    # e.g. U:1072x1448p-0
    sed -n 's/^[^:]*:\([0-9]*\)x\([0-9]*\).*/\1 \2/p' "$FB_MODES" 2>/dev/null | head -n 1
}

# KOReader rotates in software and tells nobody, so there this reads upright.
# Wrapped because a suspended framework can hang the call.
rotation() {
    [ -n "$TAP_ROTATION" ] && { echo "$TAP_ROTATION"; return; }
    if command -v timeout >/dev/null 2>&1; then
        o=$(timeout 1 lipc-get-prop com.lab126.system orientation 2>/dev/null)
    else
        o=$(lipc-get-prop com.lab126.system orientation 2>/dev/null)
    fi
    case "$o" in
        R) echo 90 ;;
        D) echo 180 ;;
        L) echo 270 ;;
        *) echo 0 ;;
    esac
}

DEV=$(touch_node)
[ -z "$DEV" ] && { echo "tap.sh: no touchscreen found" >&2; exit 1; }

SIZE=$(screen_size)
W=${SIZE% *}
H=${SIZE#* }
case "$W$H" in
    ""|*[!0-9]*) echo "tap.sh: cannot read screen size from $FB_MODES" >&2; exit 1 ;;
esac

ROT=$(rotation)
case "$ROT" in
    90)  T=$X_PCT; X_PCT=$(( 100 - Y_PCT )); Y_PCT=$T ;;
    180) X_PCT=$(( 100 - X_PCT )); Y_PCT=$(( 100 - Y_PCT )) ;;
    270) T=$X_PCT; X_PCT=$Y_PCT; Y_PCT=$(( 100 - T )) ;;
esac

# Some firmware swaps the reported mode with the rotation, the panel never does.
case "$ROT" in
    90|270) [ "$W" -gt "$H" ] && { T=$W; W=$H; H=$T; } ;;
esac

X=$(( W * X_PCT / 100 ))
Y=$(( H * Y_PCT / 100 ))

[ -n "$TAP_DRY_RUN" ] && { echo "$X $Y"; exit 0; }

# Escape text rather than raw bytes, so the whole sequence goes out in one
# write. evdev rejects a write that is not a whole number of events, and a
# byte at a time is exactly that.
byte() { printf '\\0%03o' $(( $1 & 255 )); }
le16() { byte "$1"; byte "$(( $1 >> 8 ))"; }
le32() { byte "$1"; byte "$(( $1 >> 8 ))"; byte "$(( $1 >> 16 ))"; byte "$(( $1 >> 24 ))"; }
# The kernel stamps the time itself, so the timeval goes out as zeroes.
ev() { le32 0; le32 0; le16 "$1"; le16 "$2"; le32 "$3"; }

DOWN=$(
    ev 1 330 1     # BTN_TOUCH down
    ev 3 57 0      # ABS_MT_TRACKING_ID
    ev 3 58 18     # ABS_MT_PRESSURE
    ev 3 53 "$X"   # ABS_MT_POSITION_X
    ev 3 54 "$Y"   # ABS_MT_POSITION_Y
    ev 3 48 18     # ABS_MT_TOUCH_MAJOR
    ev 3 50 18     # ABS_MT_WIDTH_MAJOR
    ev 0 0 0       # SYN_REPORT
)
UP=$(
    ev 1 330 0     # BTN_TOUCH up
    ev 3 57 -1     # ABS_MT_TRACKING_ID, contact lifted
    ev 0 0 0
)

printf '%b' "$DOWN" > "$DEV" || { echo "tap.sh: cannot write to $DEV" >&2; exit 1; }
# busybox sleep takes whole seconds only, and holding a contact for a second
# reads as a long press.
usleep 80000 2>/dev/null
printf '%b' "$UP" > "$DEV"
