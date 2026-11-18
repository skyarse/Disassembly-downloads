import pathlib
import re
import subprocess
import tomllib


def git(*args):
    return subprocess.check_output(["git", *args], text=True).splitlines()


def stable_tags(lines):
    for tag in lines:
        if re.fullmatch(r"v\d+\.\d+\.\d+", tag):
            yield tuple(map(int, tag[1:].split(".")))


manifest = tomllib.loads(pathlib.Path("Cargo.toml").read_text(encoding="utf-8"))
minimum = tuple(map(int, manifest["package"]["version"].split(".")))
existing = list(stable_tags(git("tag", "--points-at", "HEAD")))
if existing:
    version = max(existing)
else:
    previous = list(stable_tags(git("tag", "--merged", "HEAD")))
    if previous:
        major, minor, patch = max(previous)
        minimum = max(minimum, (major, minor, patch + 1))
    version = minimum
print(".".join(map(str, version)))
