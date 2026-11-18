#!/usr/bin/env bash
set -euo pipefail

test "$GITHUB_REF" = refs/heads/main
[[ "$RELEASE_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]
test "$(git rev-parse HEAD)" = "$GITHUB_SHA"
read -r _ first_parent second_parent _ < <(git rev-list --parents -n 1 HEAD)
test -n "$first_parent" && test -n "$second_parent"
test "$(cat dist/SOURCE_COMMIT)" = "$GITHUB_SHA"
(cd dist && sha256sum --check SHA256SUMS)

tag="v$RELEASE_VERSION"
if release=$(gh release view "$tag" --json isDraft,targetCommitish); then
    test "$(jq -r .targetCommitish <<<"$release")" = "$GITHUB_SHA"
    if [[ "$(jq -r .isDraft <<<"$release")" == false ]]; then exit 0; fi
else
    gh release create "$tag" --target "$GITHUB_SHA" --title "$tag" --draft --generate-notes
fi

gh release upload "$tag" --clobber \
    dist/Downloads-Triage-x86_64.AppImage dist/Downloads-Triage-x86_64.AppImage.zsync \
    dist/SHA256SUMS dist/SOURCE_COMMIT dist/BUILD_TOOLS.json dist/Cargo.lock dist/screenshot.png
gh release edit "$tag" --draft=false --latest
if [[ "$tag" == v0.1.0 ]]; then
    gh release edit "$tag" --notes-file scripts/initial-release-notes.md
fi
