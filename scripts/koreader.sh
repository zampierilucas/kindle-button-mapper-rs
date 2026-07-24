#!/bin/sh
# KOReader HTTP API wrapper script
# Usage: koreader.sh <command> [args...]

KOREADER_URL="http://localhost:8080/koreader/event"
LOG_PATH="/var/log/kindle-button-mapper.log"

# Send event to KOReader (Asynchronously in background)
send_event() {
    (
        if ! curl -s --connect-timeout 1 --max-time 1 "${KOREADER_URL}/$1" >/dev/null 2>&1; then
            echo "$(date '+%Y-%m-%d %H:%M:%S') WARN  koreader.sh: KOReader not reachable at ${KOREADER_URL} (event '$1' dropped); KOReader may be closed, or HTTP Inspector auto-start is off." >> "$LOG_PATH" 2>/dev/null
        fi
    ) &
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
