import hashlib
import json
import os
import pathlib
import urllib.request

tools = pathlib.Path("tools")
records = {}
for repository, name in [
    ("linuxdeploy/linuxdeploy", "linuxdeploy-x86_64.AppImage"),
    ("linuxdeploy/linuxdeploy-plugin-qt", "linuxdeploy-plugin-qt-x86_64.AppImage"),
]:
    headers = {"Accept": "application/vnd.github+json", "User-Agent": "downloads-triage-build"}
    if os.environ.get("GH_TOKEN"):
        headers["Authorization"] = "Bearer " + os.environ["GH_TOKEN"]
    request = urllib.request.Request(
        f"https://api.github.com/repos/{repository}/releases/tags/continuous",
        headers=headers,
    )
    with urllib.request.urlopen(request) as response:
        release = json.load(response)
    asset = next(asset for asset in release["assets"] if asset["name"] == name)
    with urllib.request.urlopen(asset["browser_download_url"]) as response:
        content = response.read()
    digest = hashlib.sha256(content).hexdigest()
    if asset.get("digest") and asset["digest"] != "sha256:" + digest:
        raise RuntimeError(f"Digest mismatch for {name}")
    (tools / name).write_bytes(content)
    records[name] = {"url": asset["browser_download_url"], "sha256": digest, "asset_id": asset["id"]}
pathlib.Path("dist/BUILD_TOOLS.json").write_text(json.dumps(records, indent=2) + "\n")
