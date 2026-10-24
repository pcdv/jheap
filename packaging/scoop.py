"""Writes jheap's scoop manifest for a release, from the checksums beside its archives.

    python3 packaging/scoop.py <version> <dir with the .zip and .zip.sha256 files>

The release publishes it as jheap.json, so that
`scoop install https://github.com/pcdv/jheap/releases/latest/download/jheap.json`
installs the latest, and `scoop update jheap` follows the later ones.
"""

import json
import sys
from pathlib import Path

REPO = "https://github.com/pcdv/jheap"
# scoop's architecture names, and the targets that give them
TARGETS = {"64bit": "x86_64-pc-windows-msvc", "arm64": "aarch64-pc-windows-msvc"}


def main(version: str, dist: Path) -> None:
    architecture = {}
    autoupdate = {}
    for arch, target in TARGETS.items():
        name = f"jheap-v{version}-{target}"
        sha = (dist / f"{name}.zip.sha256").read_text().split()[0]
        architecture[arch] = {
            "url": f"{REPO}/releases/download/v{version}/{name}.zip",
            "hash": sha,
            "extract_dir": name,
        }
        autoupdate[arch] = {
            "url": f"{REPO}/releases/download/v$version/jheap-v$version-{target}.zip",
            "extract_dir": f"jheap-v$version-{target}",
        }
    manifest = {
        "version": version,
        "description": "What holds the memory of a JVM, from its heap dump",
        "homepage": REPO,
        "license": "Apache-2.0",
        "architecture": architecture,
        "bin": "jheap.exe",
        "checkver": {"github": REPO},
        "autoupdate": {"architecture": autoupdate},
    }
    print(json.dumps(manifest, indent=4))


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], Path(sys.argv[2]))
