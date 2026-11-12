"""Smoke-test Linux icon packaging and installation without building or networking."""

import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


repo = Path(__file__).resolve().parent.parent
with tempfile.TemporaryDirectory(prefix="goko icons ") as directory:
    root = Path(directory)
    for name in ("apps/rune/assets", "target/release", "shims"):
        (root / name).mkdir(parents=True)
    for name in ("apps/rune/package-linux.sh", "apps/rune/assets/icon.png"):
        shutil.copyfile(repo / name, root / name)
    for name in ("goko-rune", "gokod"):
        binary = root / "target/release" / name
        binary.write_text("#!/bin/sh\nexit 0\n")
        binary.chmod(0o755)
    for name, body in {
        "cargo": "exit 0",
        "uname": 'case "$1" in -s) echo Linux;; -m) echo aarch64;; esac',
        "pgrep": "exit 1",
        "update-desktop-database": "exit 0",
    }.items():
        shim = root / "shims" / name
        shim.write_text("#!/bin/sh\n" + body + "\n")
        shim.chmod(0o755)
    env = dict(os.environ, PATH=f"{root / 'shims'}:{os.environ['PATH']}")
    subprocess.run(
        ["bash", str(root / "apps/rune/package-linux.sh"), "0.0.0", (root / "dist").as_uri()],
        env=env, check=True,
    )
    dist = root / "dist"
    targets = json.loads((dist / "linux-aarch64.target.json").read_text())
    (dist / "latest.json").write_text(json.dumps({"version": "0.0.0", "targets": targets}))
    env.update(
        HOME=str(root / "home"), XDG_DATA_HOME=str(root / "data"),
        GOKO_RELEASES_URL=dist.as_uri(),
    )
    subprocess.run(["sh", str(repo / "distribution/install.sh")], env=env, check=True)
    installed = root / "data/goko/icon.png"
    assert installed.read_bytes() == (repo / "apps/rune/assets/icon.png").read_bytes()
    desktop = root / "data/applications/goko.desktop"
    assert f"Icon={installed}\n" in desktop.read_text()

    # Older releases remain installable with the generic icon.
    archive = dist / "Goko-0.0.0-linux-aarch64.tar.gz"
    subprocess.run(["tar", "-xzf", str(archive), "-C", str(dist)], check=True)
    (dist / "goko/icon.png").unlink()
    subprocess.run(["tar", "-czf", str(archive), "-C", str(dist), "goko"], check=True)
    targets["linux-aarch64"]["sha256"] = hashlib.sha256(archive.read_bytes()).hexdigest()
    (dist / "latest.json").write_text(json.dumps({"version": "0.0.0", "targets": targets}))
    subprocess.run(["sh", str(repo / "distribution/install.sh")], env=env, check=True)
    assert "Icon=utilities-terminal\n" in desktop.read_text()
    assert not installed.exists()
print("PASS: Linux archive icon, installed launcher, and older-release fallback")
