#!/usr/bin/env bash
set -euo pipefail
mkdir -p tools dist
python3 scripts/fetch-tools.py
chmod +x tools/*.AppImage
export APPIMAGE_EXTRACT_AND_RUN=1
export PATH="$PWD/tools:$PATH"
export QMAKE=/usr/bin/qmake6
export QML_SOURCES_PATHS="$PWD/qml"
export EXTRA_PLATFORM_PLUGINS="libqwayland-egl.so;libqwayland-generic.so"
export EXTRA_QT_MODULES="Svg"
export OUTPUT="$PWD/dist/Downloads-Triage-x86_64.AppImage"
python3 scripts/licenses.py
tools/linuxdeploy-x86_64.AppImage --appdir AppDir --executable target/release/downloads-triage --desktop-file packaging/net.skyarse.DownloadsTriage.desktop --icon-file packaging/net.skyarse.DownloadsTriage.svg --plugin qt --output appimage
sha256sum dist/*.AppImage | sed 's|  dist/|  |' > dist/SHA256SUMS
cp Cargo.lock dist/Cargo.lock
git rev-parse HEAD > dist/SOURCE_COMMIT
