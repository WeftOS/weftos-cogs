#!/usr/bin/env python3
"""Attestations for a weft-cog-host release (called by scripts/cog-host-release.sh).

Reads <out>/weft-cog-host-<target> and the .glibc-<target> / .rustc notes the build left, then
writes, inside <out>:

  sbom.cdx.json            CycloneDX 1.5 SBOM for crates/cog-host (cargo-cyclonedx, all targets)
  provenance.intoto.json   in-toto v1 Statement, SLSA provenance v1 (binaries + SBOM as subjects)
  cog-host-manifest.json   version, commit, per binary target/size/sha256/glibc floor/builder digest
  SHA256SUMS               sha256sum format over the binaries, SBOM, provenance and manifest

  scripts/cog-host-attest.py --out release/cog-host --version v0.1.3 --image <builder image> \\
      --build-started <iso> [--build-finished <iso>] [--allow-dirty] <target>...

Helpers (git source URL, image digest, dirty check) are the ones scripts/release-attest.py uses.
Fails if a binary needs a glibc newer than GLIBC_MAX. Paths in the output are relative to <out>.
"""
import argparse
import importlib.util
import json
import re
import shutil
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
_spec = importlib.util.spec_from_file_location("release_attest", ROOT / "scripts" / "release-attest.py")
ra = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(ra)

CRATE = ROOT / "crates" / "cog-host"
# Debian bookworm / Raspberry Pi OS bookworm ship 2.36; Ubuntu 24.04 ships 2.39.
GLIBC_MAX = (2, 36)


def glibc_floor(note):
    versions = [tuple(int(x) for x in v.removeprefix("GLIBC_").split("."))
                for v in note.read_text().split() if re.fullmatch(r"GLIBC_\d+(\.\d+)*", v)]
    return max(versions) if versions else None


def make_sbom(out):
    # cargo-cyclonedx has no package filter: it writes sbom.cdx.json beside every workspace
    # member. Keep cog-host's and remove the ones this run created.
    before = set(ROOT.glob("crates/*/sbom.cdx.json"))
    ra.run(["cargo", "cyclonedx", "--format", "json", "--spec-version", "1.5",
            "--target", "all", "--override-filename", "sbom.cdx", "--quiet"], cwd=CRATE)
    produced = CRATE / "sbom.cdx.json"
    for extra in set(ROOT.glob("crates/*/sbom.cdx.json")) - before - {produced}:
        extra.unlink()
    if not produced.is_file():
        ra.die("cargo-cyclonedx wrote no SBOM for cog-host")
    dest = out / "sbom.cdx.json"
    shutil.move(str(produced), dest)
    # cargo-cyclonedx writes bom-refs as path+file://<absolute checkout>; keep host paths out.
    dest.write_text(dest.read_text().replace(f"file://{ROOT}/", "file://./"))
    doc = json.loads(dest.read_text())
    if doc.get("bomFormat") != "CycloneDX" or doc.get("specVersion") != "1.5":
        ra.die("cog-host SBOM is not CycloneDX 1.5")
    if doc["metadata"]["component"]["name"] != "cog-host":
        ra.die("cog-host SBOM describes the wrong component")
    return dest


def entry(out, p):
    return {"path": str(p.relative_to(out)), "sha256": ra.sha256_file(p), "size": p.stat().st_size}


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--out", required=True)
    ap.add_argument("--version", required=True, help="release tag, e.g. v0.1.3")
    ap.add_argument("--image", required=True, help="cross-build container image name")
    ap.add_argument("--build-started", required=True)
    ap.add_argument("--build-finished", default=None)
    ap.add_argument("--allow-dirty", action="store_true")
    ap.add_argument("targets", nargs="+")
    a = ap.parse_args()

    if not re.fullmatch(r"v\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?", a.version):
        ra.die(f"--version {a.version!r} is not a tag like v0.1.3")
    out = Path(a.out).resolve()
    dirty = ra.tree_is_dirty()
    if dirty and not a.allow_dirty:
        ra.die("git tree is dirty; commit or stash, or pass --allow-dirty (the output is then marked)")
    commit = ra.run(["git", "rev-parse", "HEAD"]).strip()
    url = ra.source_url()
    digest = ra.image_digest(a.image)
    rustc = (out / ".rustc").read_text().strip()
    crate_version = json.loads(ra.run(["cargo", "metadata", "--format-version", "1", "--no-deps",
                                       "--locked"]))
    crate_version = next(p["version"] for p in crate_version["packages"] if p["name"] == "cog-host")

    binaries = []
    for t in a.targets:
        p = out / f"weft-cog-host-{t}"
        if not p.is_file():
            ra.die(f"missing {p.name}")
        floor = glibc_floor(out / f".glibc-{t}")
        if floor is None or floor > GLIBC_MAX:
            ra.die(f"{p.name} needs GLIBC_{'.'.join(map(str, floor or ()))}, above "
                   f"{'.'.join(map(str, GLIBC_MAX))}")
        binaries.append({**entry(out, p), "target": t,
                         "glibc_min": "GLIBC_" + ".".join(map(str, floor)),
                         "builder": {"image": a.image, "digest": {"sha256": digest}}})

    sbom = make_sbom(out)
    if not a.allow_dirty and ra.tree_is_dirty():
        ra.die("SBOM generation changed the git tree (Cargo.lock out of date?)")
    sbom_entry = entry(out, sbom)

    finished = a.build_finished or ra.now()
    source_dep = {"uri": f"git+{url}@{commit}", "digest": {"gitCommit": commit}}
    if dirty:
        source_dep["annotations"] = {"dirty": True}
    provenance = {
        "_type": ra.STATEMENT,
        "subject": [{"name": e["path"], "digest": {"sha256": e["sha256"]}}
                    for e in binaries + [sbom_entry]],
        "predicateType": ra.PREDICATE,
        "predicate": {
            "buildDefinition": {
                "buildType": f"{url}/blob/{commit}/scripts/cog-host-release.sh",
                "externalParameters": {
                    "source": {"uri": f"git+{url}", "digest": {"gitCommit": commit}},
                    "version": a.version,
                    "entryPoint": "scripts/cog-host-release.sh",
                    "package": "cog-host",
                    "binary": "weft-cog-host",
                },
                "internalParameters": {
                    "allowDirty": a.allow_dirty,
                    "dirtyTree": dirty,
                    "targets": a.targets,
                    "cargo": "cargo build --locked --release -p cog-host --bin weft-cog-host",
                    "rustc": rustc,
                    "sbomTool": ra.run(["cargo", "cyclonedx", "--version"]).strip(),
                },
                "resolvedDependencies": [source_dep],
            },
            "runDetails": {
                "builder": {
                    "id": f"{url}/blob/{commit}/scripts/cog-host-release.sh#controlled-host",
                    "builderDependencies": [{
                        "name": a.image,
                        "uri": f"docker-image:{a.image}",
                        "digest": {"sha256": digest},
                    }],
                },
                "metadata": {
                    "invocationId": str(uuid.uuid4()),
                    "startedOn": a.build_started,
                    "finishedOn": finished,
                },
            },
        },
    }
    prov_path = out / "provenance.intoto.json"
    ra.write_json(prov_path, provenance)

    manifest = {
        "schema": 1,
        "kind": "weavelogic-cog-host-release",
        "version": a.version,
        "crate_version": crate_version,
        "commit": commit,
        "source": url,
        "dirty": dirty,
        "created": ra.now(),
        "binaries": binaries,
        "sbom": sbom_entry,
        "provenance": entry(out, prov_path),
    }
    man_path = out / "cog-host-manifest.json"
    ra.write_json(man_path, manifest)

    for note in out.glob(".*"):
        note.unlink()
    lines = [f"{ra.sha256_file(out / e['path'])}  {e['path']}"
             for e in binaries + [sbom_entry, entry(out, prov_path), entry(out, man_path)]]
    (out / "SHA256SUMS").write_text("\n".join(lines) + "\n")
    print(f"cog-host-attest: {a.version} {commit[:12]}{' DIRTY' if dirty else ''}: "
          f"{len(binaries)} binaries, SBOM, provenance, manifest, SHA256SUMS")
    for b in binaries:
        print(f"  {b['path']:<48} {b['size']:>9}  {b['glibc_min']:<11} {b['sha256']}")


if __name__ == "__main__":
    main()
