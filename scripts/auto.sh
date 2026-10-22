#!/bin/sh
# Reader control that follows whatever is on screen.
# Usage: auto.sh <command> [args...]
#
# Commands:
#   next_page         - Turn to next page
#   prev_page         - Turn to previous page
#   next_page_tap     - Next page, tapping the screen on the native reader
#   prev_page_tap     - Previous page, tapping the screen on the native reader
#   brightness <n>    - Adjust frontlight (positive=up, negative=down)
#   brightness_toggle - Toggle frontlight off/on
#   dark_mode_toggle  - KOReader night mode, native system dark mode
#   menu              - KOReader menu, native toolbar
#
# KOReader answers on its HTTP Inspector port when it is up, so a delivered
# event means it is the reader in front of you. Trying it is the probe — a
# separate one would only cost another curl. Everything else, including the
# native-only commands (home, back, toolbar), goes to kindle.sh. The tap
# variants only change how the native side turns the page, KOReader still gets
# the HTTP event.

DIR=$(dirname "$0")

try_koreader() {
    KBM_QUIET=1 "$DIR/koreader.sh" "$@"
}

CMD="$1"
case "$CMD" in
    next_page|prev_page|brightness|brightness_toggle|font_up|font_down|toggle_status_bar|rotate)
        try_koreader "$@" && exit 0
        ;;
    next_page_tap)
        try_koreader next_page && exit 0
        ;;
    prev_page_tap)
        try_koreader prev_page && exit 0
        ;;
    dark_mode_toggle|night_mode)
        try_koreader night_mode && exit 0
        exec "$DIR/kindle.sh" dark_mode_toggle
        ;;
    menu)
        try_koreader menu && exit 0
        exec "$DIR/kindle.sh" toolbar
        ;;
    "")
        echo "Usage: $0 <command> [args...]"
        echo "Commands: next_page, prev_page, next_page_tap, prev_page_tap,"
        echo "          brightness <n>, brightness_toggle, dark_mode_toggle, menu"
        exit 1
        ;;
esac

exec "$DIR/kindle.sh" "$@"
