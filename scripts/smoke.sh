#!/usr/bin/env bash
set -euo pipefail
if [[ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]]; then exec dbus-run-session -- bash "$0" "$@"; fi
smoke_case="${DOWNLOADS_UI_CASE:-queue}"
smoke_root="$(mktemp -d)"
smoke_pid=""
weston_pid=""
watcher_pid=""
cleanup() {
    if [[ -n "$smoke_pid" ]]; then kill "$smoke_pid" 2>/dev/null || true; fi
    if [[ -n "$weston_pid" ]]; then kill "$weston_pid" 2>/dev/null || true; fi
    if [[ -n "$watcher_pid" ]]; then kill "$watcher_pid" 2>/dev/null || true; fi
    rm -rf "$smoke_root"
}
trap cleanup EXIT
export XDG_CONFIG_HOME="$smoke_root/config"
export XDG_DATA_HOME="$smoke_root/data"
export XDG_RUNTIME_DIR="$smoke_root/runtime"
mkdir -m700 "$XDG_RUNTIME_DIR"
python3 - <<'PY'
import json, os, pathlib
root = pathlib.Path(os.environ["XDG_RUNTIME_DIR"]).parent / "Downloads"
root.mkdir()
(root / "Screenshot.png").write_bytes(b"preview fixture")
(root / "unknown.iso").write_bytes(b"preview fixture")
(root / "unfinished.png.part").write_bytes(b"partial fixture")
config_base = pathlib.Path(os.environ["XDG_CONFIG_HOME"])
config_base.mkdir(parents=True)
(config_base / "user-dirs.dirs").write_text('XDG_DOWNLOAD_DIR="' + str(root) + '"\n')
config = config_base / "downloads-triage"
config.mkdir(parents=True)
settings = {
    "version": 1, "root": str(root),
    "categories": [{"id": "pictures", "name": "Картинки", "folder": "Картинки"}],
    "rules": [{"id": "pictures", "name": "Изображения", "condition": {"extensions": ["png"], "pattern": ""}, "category_id": "pictures", "action": "move", "active": True}],
    "exclusions": [],
}
if os.environ.get("DOWNLOADS_UI_CASE") != "first-run":
    (config / "settings.json").write_text(json.dumps(settings))
PY
python3 tests/tray_watcher.py "$smoke_root/tray-item" > "$smoke_root/tray.log" 2>&1 &
watcher_pid=$!
for attempt in $(seq 1 20); do
    if [[ -f "$smoke_root/tray-item.ready" ]]; then break; fi
    sleep 0.1
 done
if [[ ! -f "$smoke_root/tray-item.ready" ]]; then cat "$smoke_root/tray.log"; exit 1; fi
export QT_QUICK_CONTROLS_STYLE=Basic
export QT_QUICK_BACKEND=software
export QT_IM_MODULE=compose
export WAYLAND_DISPLAY=wayland-smoke
weston --backend=headless-backend.so --socket="$WAYLAND_DISPLAY" --idle-time=0 --width=1280 --height=800 > "$smoke_root/weston.log" 2>&1 &
weston_pid=$!
for attempt in $(seq 1 15); do
    if [[ -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ]]; then break; fi
    sleep 1
done
if [[ ! -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ]]; then cat "$smoke_root/weston.log"; exit 1; fi
export QT_QPA_PLATFORM=wayland
if [[ $# -eq 0 ]]; then set -- target/release/downloads-triage; fi
mkdir -p dist
smoke_options=()
smoke_image="screenshot.png"
if [[ "$smoke_case" == "first-run" ]]; then
    smoke_options=(--first-run-test --test-width 880)
    smoke_image="settings-first-run.png"
elif [[ "$smoke_case" == "automation" ]]; then
    smoke_options=(--automation-test)
elif [[ "$smoke_case" == "dark" ]]; then
    smoke_options=(--test-width 880 --test-theme dark)
    smoke_image="screenshot-dark.png"
fi
"$@" --smoke-test --screenshot "$PWD/dist/$smoke_image" "${smoke_options[@]}" > "$smoke_root/qt.log" 2>&1 &
smoke_pid=$!
smoke_limit=35
if [[ "$smoke_case" == "automation" ]]; then smoke_limit=100; fi
for attempt in $(seq 1 "$smoke_limit"); do
    if ! kill -0 "$smoke_pid" 2>/dev/null; then break; fi
    sleep 1
done
if kill -0 "$smoke_pid" 2>/dev/null; then cat "$smoke_root/qt.log"; exit 1; fi
if ! wait "$smoke_pid"; then
    cat "$smoke_root/qt.log"
    gdb --batch -ex run -ex "thread apply all bt" --args "$@" --smoke-test || true
    exit 1
fi
smoke_pid=""
cat "$smoke_root/qt.log"
if [[ "$smoke_case" == "automation" ]]; then
    rg 'AUTOMATION_READY' "$smoke_root/qt.log"
    test -f "$smoke_root/Downloads/Картинки/Screenshot.png"
    test ! -e "$smoke_root/Downloads/Screenshot.png"
    test -f "$smoke_root/Downloads/unknown.iso"
    test -f "$smoke_root/Downloads/unfinished.png.part"
else
rg 'QML_READY' "$smoke_root/qt.log"
rg 'UI_DIALOGS_READY' "$smoke_root/qt.log"
rg 'BRIDGE_REPEAT_READY' "$smoke_root/qt.log"
rg 'UI_LAYOUT_READY' "$smoke_root/qt.log"
rg 'UI_TRAY_READY' "$smoke_root/qt.log"
fi
test -s "$smoke_root/tray-item"
if rg 'failed to load|Failed to load|is not installed|ReferenceError|TypeError|Binding loop|Cannot assign|SCREENSHOT_FAILED|UI_SMOKE_FAILED' "$smoke_root/qt.log"; then exit 1; fi
