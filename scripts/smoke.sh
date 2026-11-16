#!/usr/bin/env bash
set -euo pipefail
smoke_root="$(mktemp -d)"
trap 'rm -rf "$smoke_root"' EXIT
export XDG_CONFIG_HOME="$smoke_root/config"
export XDG_DATA_HOME="$smoke_root/data"
export QT_QUICK_CONTROLS_STYLE=Fusion
export QT_QPA_PLATFORM=offscreen
target/release/downloads-triage --smoke-test > "$smoke_root/qt.log" 2>&1 &
smoke_pid=$!
for attempt in $(seq 1 30); do
    if ! kill -0 "$smoke_pid" 2>/dev/null; then break; fi
    sleep 1
done
if kill -0 "$smoke_pid" 2>/dev/null; then
    kill "$smoke_pid"
    cat "$smoke_root/qt.log"
    exit 1
fi
wait "$smoke_pid"
cat "$smoke_root/qt.log"
rg 'QML_READY' "$smoke_root/qt.log"
if rg 'failed to load|is not installed|ReferenceError|TypeError|Binding loop' "$smoke_root/qt.log"; then exit 1; fi
