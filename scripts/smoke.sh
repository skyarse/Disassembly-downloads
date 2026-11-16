#!/usr/bin/env bash
set -euo pipefail
smoke_root="$(mktemp -d)"
smoke_pid=""
weston_pid=""
cleanup() {
    if [[ -n "$smoke_pid" ]]; then kill "$smoke_pid" 2>/dev/null || true; fi
    if [[ -n "$weston_pid" ]]; then kill "$weston_pid" 2>/dev/null || true; fi
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
config = pathlib.Path(os.environ["XDG_CONFIG_HOME"]) / "downloads-triage"
config.mkdir(parents=True)
settings = {
    "version": 1, "root": str(root),
    "categories": [{"id": "pictures", "name": "Картинки", "folder": "Картинки"}],
    "rules": [{"id": "pictures", "name": "Изображения", "condition": {"extensions": ["png"], "pattern": ""}, "category_id": "pictures", "action": "move", "active": True}],
    "exclusions": [],
}
(config / "settings.json").write_text(json.dumps(settings))
PY
export QT_QUICK_CONTROLS_STYLE=Fusion
export QT_QUICK_BACKEND=software
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
"$@" --smoke-test --screenshot "$PWD/dist/screenshot.png" > "$smoke_root/qt.log" 2>&1 &
smoke_pid=$!
for attempt in $(seq 1 30); do
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
rg 'QML_READY' "$smoke_root/qt.log"
if rg 'failed to load|is not installed|ReferenceError|TypeError|Binding loop|Cannot assign|SCREENSHOT_FAILED|UI_SMOKE_FAILED' "$smoke_root/qt.log"; then exit 1; fi
