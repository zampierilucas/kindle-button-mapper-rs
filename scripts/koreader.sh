#!/bin/sh
# KOReader HTTP API wrapper script
# Usage: koreader.sh <command> [args...]
#
# Commands:
#   next_page        - Turn to next page
#   prev_page        - Turn to previous page
#   brightness <n>   - Adjust brightness (positive=up, negative=down)
#   brightness_toggle - Toggle frontlight on/off
#   night_mode       - Toggle night/dark mode
#   font_up [n]      - Increase font size (default: 1)
#   font_down [n]    - Decrease font size (default: 1)
#   event <name> [args] - Send arbitrary event

KOREADER_PORTS="8323 8080"
LOG_PATH="/var/log/kindle-button-mapper.log"

# Send event to KOReader. Exits non-zero when it did not land, which is what
# lets auto.sh fall back to the native reader. KBM_QUIET=1 silences the warning
# for callers that have a fallback.
send_event() {
    for port in $KOREADER_PORTS; do
        if curl -s --connect-timeout 1 "http://localhost:${port}/koreader/event/$1" >/dev/null 2>&1; then
            return 0
        fi
    done
    [ -z "$KBM_QUIET" ] && echo "$(date '+%Y-%m-%d %H:%M:%S') WARN  koreader.sh: KOReader not reachable on ports ${KOREADER_PORTS} (event '$1' dropped); KOReader may be closed, or its HID Passthrough plugin is missing." >> "$LOG_PATH" 2>/dev/null
    return 1
}

case "$1" in
    next_page)
        send_event "GotoViewRel/1"
        ;;
    prev_page)
        send_event "GotoViewRel/-1"
        ;;
    brightness)
        step="${2:-1}"
        if [ "$step" -gt 0 ] 2>/dev/null; then
            send_event "IncreaseFlIntensity/${step}"
        elif [ "$step" -lt 0 ] 2>/dev/null; then
            step=$(echo "$step" | tr -d '-')
            send_event "DecreaseFlIntensity/${step}"
        fi
        ;;
    brightness_toggle)
        send_event "ToggleFrontlight"
        ;;
    night_mode)
        send_event "ToggleNightMode"
        ;;
    font_up)
        step="${2:-1}"
        send_event "IncreaseFontSize/${step}"
        ;;
    font_down)
        step="${2:-1}"
        send_event "DecreaseFontSize/${step}"
        ;;
    menu)
        send_event "ShowMenu"
        ;;
    toggle_status_bar)
        send_event "ToggleFooterMode"
        ;;
    rotate)
        send_event "IterateRotation"
        ;;
    event)
        shift
        send_event "$*"
        ;;
    *)
        echo "Usage: $0 <command> [args...]"
        echo "Commands: next_page, prev_page, brightness <n>, brightness_toggle,"
        echo "          night_mode, font_up [n], font_down [n], menu, toggle_status_bar,"
        echo "          rotate, event <name> [args]"
        exit 1
        ;;
esac
