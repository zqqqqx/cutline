"""Erzeugt index.json für das Release „packs“.

    python tools/index.py <paketwurzel> <zip-ordner>

Liest jedes <paketwurzel>/<ordner>/plugin.toml und die fertige <zip-ordner>/<ordner>.zip und
schreibt <zip-ordner>/index.json (Format siehe README.md).
"""
import hashlib
import json
import pathlib
import sys
import tomllib


def eintrag(ordner, zp):
    m = tomllib.loads((ordner / "plugin.toml").read_text(encoding="utf-8"))
    p = m["plugin"]
    arten = {}
    for a in m.get("asset", []):
        arten[a["kind"]] = arten.get(a["kind"], 0) + 1
    data = zp.read_bytes()
    return {
        "id": p["id"],
        "name": p["name"],
        "version": p["version"],
        "api": p["api"],
        "description": p.get("description", {}),
        "file": zp.name,
        "size": len(data),
        "sha256": hashlib.sha256(data).hexdigest(),
        "counts": dict(sorted(arten.items())),
        "license": p["license"],
    }


def main():
    wurzel, zips = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
    pakete = []
    for ordner in sorted(d for d in wurzel.iterdir() if (d / "plugin.toml").is_file()):
        zp = zips / f"{ordner.name}.zip"
        if not zp.is_file():
            sys.exit(f"ZIP fehlt: {zp}")
        pakete.append(eintrag(ordner, zp))
    ids = [p["id"] for p in pakete]
    assert len(ids) == len(set(ids)), "doppelte Paket-IDs"
    out = {"format": 1, "packs": pakete}
    (zips / "index.json").write_text(json.dumps(out, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"index.json: {len(pakete)} Pakete")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main()
