import json
import pathlib
import shutil
import subprocess

destination = pathlib.Path("AppDir/usr/share/licenses/downloads-triage")
destination.mkdir(parents=True, exist_ok=True)
shutil.copy2("LICENSE", destination / "LICENSE")
metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--locked", "--format-version=1"]))
records = []
for package in metadata["packages"]:
    root = pathlib.Path(package["manifest_path"]).parent
    output = destination / "rust" / (package["name"] + "-" + package["version"])
    output.mkdir(parents=True, exist_ok=True)
    for path in root.iterdir():
        if path.is_file() and path.name.lower().startswith(("license", "copying", "copyright", "notice")):
            shutil.copy2(path, output / path.name)
    records.append({key: package.get(key) for key in ["name", "version", "license", "repository", "source"]})
(destination / "rust-packages.json").write_text(json.dumps(records, indent=2) + "\n")
for path in pathlib.Path("/usr/share/doc").glob("*qt6*/copyright"):
    output = destination / "qt" / path.parent.name
    output.mkdir(parents=True, exist_ok=True)
    shutil.copy2(path, output / "copyright")
