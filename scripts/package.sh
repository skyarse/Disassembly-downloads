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
export EXTRA_QT_MODULES="svg;waylandcompositor"
export OUTPUT="$PWD/dist/Downloads-Triage-x86_64.AppImage"
if [[ -n "${DOWNLOADS_TRIAGE_VERSION:-}" ]]; then
    export LDAI_VERSION="$DOWNLOADS_TRIAGE_VERSION"
    export LDAI_OUTPUT="$OUTPUT"
    export LDAI_UPDATE_INFORMATION='gh-releases-zsync|skyarse|Disassembly-downloads|latest|Downloads-Triage-x86_64.AppImage.zsync'
fi
python3 scripts/licenses.py
project="$PWD"
(
    # Some appimagetool builds write the zsync file in the working directory.
    cd dist
    "$project/tools/linuxdeploy-x86_64.AppImage" --appdir "$project/AppDir" \
        --executable "$project/target/release/downloads-triage" \
        --desktop-file "$project/packaging/net.skyarse.DownloadsTriage.desktop" \
        --icon-file "$project/packaging/net.skyarse.DownloadsTriage.svg" --plugin qt --output appimage
    if [[ -n "${DOWNLOADS_TRIAGE_VERSION:-}" ]]; then
        embedded=$(env -u APPIMAGE_EXTRACT_AND_RUN ./Downloads-Triage-x86_64.AppImage --appimage-updateinformation)
        test "$embedded" = "$LDAI_UPDATE_INFORMATION"
        rg -aqx 'URL: Downloads-Triage-x86_64.AppImage' Downloads-Triage-x86_64.AppImage.zsync
        sha256sum Downloads-Triage-x86_64.AppImage Downloads-Triage-x86_64.AppImage.zsync > SHA256SUMS
    else
        sha256sum Downloads-Triage-x86_64.AppImage > SHA256SUMS
    fi
)
cp Cargo.lock dist/Cargo.lock
git rev-parse HEAD > dist/SOURCE_COMMIT
