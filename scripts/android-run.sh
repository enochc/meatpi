#!/usr/bin/env bash
# Build MeatPi and install it on the Android device you choose.
#
#     scripts/android-run.sh            # pick from the connected devices
#     scripts/android-run.sh SERIAL     # skip the list (serial from `adb devices`)
#
# Plain `cargo apk run` installs on whatever adb picks, which with the NXP
# board on ADB over TCP is easily the wrong one. This lists every device by
# maker and model, asks, and pins adb to your choice with ANDROID_SERIAL.
set -euo pipefail
cd "$(dirname "$0")/.."

# shellcheck source=android-env.sh
source scripts/android-env.sh >/dev/null

ADB="$(command -v adb || true)"
[ -n "$ADB" ] || ADB="$ANDROID_HOME/platform-tools/adb"
if [ ! -x "$ADB" ]; then
    echo "adb not found. Install the Android SDK platform-tools." >&2
    exit 1
fi

prop() { "$ADB" -s "$1" shell getprop "$2" 2>/dev/null | tr -d '\r'; }

serials=()
labels=()
while read -r serial state _; do
    [ -n "$serial" ] || continue
    if [ "$state" != "device" ]; then
        echo "Skipping $serial ($state)" >&2
        continue
    fi
    serials+=("$serial")
    labels+=("$(prop "$serial" ro.product.manufacturer) $(prop "$serial" ro.product.model)  [$serial]")
done < <("$ADB" devices | tail -n +2)

count=${#serials[@]}
target=""

if [ $# -gt 0 ] && [ "$1" != "--" ]; then
    target="$1"
    shift
    found=""
    for i in $(seq 0 $((count - 1))); do
        [ "${serials[$i]}" = "$target" ] && found="${labels[$i]}"
    done
    if [ -z "$found" ]; then
        echo "$target is not a connected device. Run 'adb devices' to see the list." >&2
        exit 1
    fi
    echo "Installing on $found"
elif [ "$count" -eq 0 ]; then
    echo "No Android devices connected. Plug in the phone and allow USB debugging." >&2
    exit 1
elif [ "$count" -eq 1 ]; then
    printf 'Install on %s? [y/N] ' "${labels[0]}"
    read -r answer
    case "$answer" in
        y|Y|yes|YES) target="${serials[0]}" ;;
        *) echo "Cancelled."; exit 1 ;;
    esac
else
    echo "Connected devices:"
    for i in $(seq 0 $((count - 1))); do
        echo "  $((i + 1))) ${labels[$i]}"
    done
    printf 'Install on which device? [1-%d] ' "$count"
    read -r choice
    case "$choice" in
        ''|*[!0-9]*) echo "Cancelled."; exit 1 ;;
    esac
    if [ "$choice" -lt 1 ] || [ "$choice" -gt "$count" ]; then
        echo "Cancelled."
        exit 1
    fi
    target="${serials[$((choice - 1))]}"
fi

[ "${1:-}" = "--" ] && shift
export ANDROID_SERIAL="$target"
exec cargo apk run -p meatpi-app --lib "$@"
