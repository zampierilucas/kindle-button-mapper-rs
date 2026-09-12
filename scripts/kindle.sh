#!/bin/sh
# Native Kindle reader control.
# Usage: kindle.sh <command> [args...]
#
# Commands:
#   next_page         - Turn to next page
#   prev_page         - Turn to previous page
#   next_page_tap     - Turn to next page by tapping the screen
#   prev_page_tap     - Turn to previous page by tapping the screen
#   home              - Go to the home screen
#   back              - Back / close the current view
#   toolbar           - Toggle the reader toolbar
#   brightness <n>    - Adjust frontlight (positive=up, negative=down)
#   brightness_toggle - Toggle frontlight off/on
#   suspend           - Put the device to sleep
#
# Page turns are handed to the daemon, which injects into the physical page
# buttons on models that have them and falls back to KEY_DOWN/KEY_UP on its
# virtual keyboard everywhere else. Where there is no key path at all, they tap
# instead. The tap variants force that for readers that take the keys but
# ignore them, which nothing here can tell apart from a key that worked. Both
# go through tap.sh. The rest is lipc.

DIR=$(dirname "$0")
LOG_PATH="/var/log/kindle-button-mapper.log"
FL_SAVED="/var/run/kindle-button-mapper-fl"
SUSPEND_STAMP="/var/run/kindle-button-mapper-suspend"

warn() {
    echo "$(date '+%Y-%m-%d %H:%M:%S') WARN  kindle.sh: $1" >> "$LOG_PATH" 2>/dev/null
}

send_key() {
    err=$("$DIR/key.sh" "$1" 2>&1) && return 0
    warn "cannot inject $1: ${err:-unknown error}"
    return 1
}

# Quiet, since the caller falls back to a tap and a device with no key path at
# all would otherwise log a line on every page turn. The daemon already says
# why once at startup.
try_key() {
    "$DIR/key.sh" "$1" >/dev/null 2>&1
}

# Right edge for next, left edge for previous, both at mid height.
tap_page() {
    "$DIR/tap.sh" "$2" 50 || warn "$1: cannot tap the screen"
}

fl_get() { lipc-get-prop com.lab126.powerd flIntensity 2>/dev/null || echo 0; }
fl_max() { lipc-get-prop com.lab126.powerd flMaxIntensity 2>/dev/null || echo 24; }
fl_set() { lipc-set-prop com.lab126.powerd flIntensity "$1" 2>/dev/null; }

case "$1" in
    next_page)
        try_key page_next || tap_page next_page 85
        ;;
    prev_page)
        try_key page_prev || tap_page prev_page 12
        ;;
    next_page_tap)
        tap_page next_page_tap 85
        ;;
    prev_page_tap)
        tap_page prev_page_tap 12
        ;;
    home)
        send_key KEY_HOME
        ;;
    back)
        lipc-set-prop com.lab126.appmgrd kppBackButtonPress 1 2>/dev/null \
            || warn "back: appmgrd not reachable"
        ;;
    toolbar)
        state=$(lipc-get-prop com.lab126.winmgr chromeState 2>/dev/null || echo 0)
        if [ "$state" = "0" ]; then
            lipc-set-prop com.lab126.winmgr chromeState 1 2>/dev/null
        else
            lipc-set-prop com.lab126.winmgr chromeState 0 2>/dev/null
        fi
        ;;
    brightness)
        step="${2:-1}"
        max=$(fl_max)
        new=$(( $(fl_get) + step ))
        [ "$new" -lt 0 ] && new=0
        [ "$new" -gt "$max" ] && new="$max"
        fl_set "$new"
        ;;
    brightness_toggle)
        cur=$(fl_get)
        if [ "$cur" -gt 0 ]; then
            echo "$cur" > "$FL_SAVED" 2>/dev/null
            fl_set 0
        else
            prev=$(cat "$FL_SAVED" 2>/dev/null)
            case "$prev" in
                ""|0|*[!0-9]*) prev=$(( $(fl_max) / 2 )) ;;
            esac
            fl_set "$prev"
        fi
        ;;
    suspend)
        # powerButton is a toggle and a held button repeats its action, so
        # calls within 2s of the last are dropped: one press, one transition.
        now=$(date +%s)
        last=$(cat "$SUSPEND_STAMP" 2>/dev/null)
        case "$last" in ''|*[!0-9]*) last=0 ;; esac
        if [ $(( now - last )) -ge 2 ]; then
            echo "$now" > "$SUSPEND_STAMP" 2>/dev/null
            lipc-set-prop com.lab126.powerd powerButton 1 2>/dev/null \
                || warn "suspend: powerd not reachable"
        fi
        ;;
    *)
        echo "Usage: $0 <command> [args...]"
        echo "Commands: next_page, prev_page, next_page_tap, prev_page_tap,"
        echo "          home, back, toolbar,"
        echo "          brightness <n>, brightness_toggle, suspend"
        exit 1
        ;;
esac
