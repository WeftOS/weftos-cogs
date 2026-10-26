#!/usr/bin/env python3
"""Verify a cogs release directory: signatures, checksums, SBOMs, provenance.

  scripts/release-verify.py <repo-dir> [--bin weft-cog-repo] [--allow-dirty] [--pin <pubkey-hex>]...

<repo-dir> holds registry.json, release-manifest.json(.sig), provenance.intoto.json(.sig) and
the artifact tree, as downloaded from a release. Signatures are checked with
`weft-cog-repo verify-file` against the compiled-in WeaveLogic key (or each --pin).
Per-binary signatures are the job of `weft-cog-repo verify <repo-dir>`; run that too.
Exits non-zero on the first class of failure it finds, after listing every problem it saw.
"""
import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path


def sha256_file(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("repo")
    ap.add_argument("--bin", default="weft-cog-repo")
    ap.add_argument("--allow-dirty", action="store_true")
    ap.add_argument("--pin", action="append", default=[])
    a = ap.parse_args()
    repo = Path(a.repo)
    problems = []

    def bad(msg):
        problems.append(msg)

    for name in ("release-manifest.json", "provenance.intoto.json"):
        cmd = [a.bin, "verify-file", str(repo / name)]
        for k in a.pin:
            cmd += ["--pin", k]
        r = subprocess.run(cmd, capture_output=True, text=True)
        if r.returncode != 0:
            bad(f"{name}: signature: {(r.stderr or r.stdout).strip()}")

    try:
        manifest = json.loads((repo / "release-manifest.json").read_text())
        prov = json.loads((repo / "provenance.intoto.json").read_text())
        registry = json.loads((repo / "registry.json").read_text())
    except (OSError, ValueError) as e:
        print(f"release-verify: FAILED: {e}", file=sys.stderr)
        return 1

    listed = {"registry.json", "release-manifest.json", "provenance.intoto.json"}

    def check_file(entry, what):
        p = repo / entry["path"]
        listed.add(entry["path"])
        if not p.is_file():
            bad(f"{what} {entry['path']}: missing")
            return
        if sha256_file(p) != entry["sha256"]:
            bad(f"{what} {entry['path']}: sha256 differs from release-manifest.json")
        if "size" in entry and p.stat().st_size != entry["size"]:
            bad(f"{what} {entry['path']}: size differs")

    check_file(manifest["registry"], "registry")
    check_file(manifest["provenance"], "provenance")
    for group in ("artifacts", "manifests", "sboms"):
        for e in manifest[group]:
            check_file(e, group[:-1])

    # registry.json agrees with the manifest, entry for entry.
    in_manifest = {e["path"]: e["sha256"] for e in manifest["artifacts"]}
    for cog in registry["cogs"]:
        for arch, art in cog["artifacts"].items():
            if in_manifest.pop(art["path"], None) != art["sha256"]:
                bad(f"{cog['id']} [{arch}]: registry artifact not in release-manifest.json "
                    f"with the same sha256")
    for left in in_manifest:
        bad(f"{left}: in release-manifest.json but not in registry.json")

    # Provenance subjects are exactly the artifacts and SBOMs.
    want = {e["path"]: e["sha256"] for e in manifest["artifacts"] + manifest["sboms"]}
    got = {s["name"]: s["digest"]["sha256"] for s in prov["subject"]}
    if want != got:
        bad("provenance subjects differ from release-manifest.json artifacts + SBOMs")
    src = prov["predicate"]["buildDefinition"]["externalParameters"]["source"]
    if src["digest"]["gitCommit"] != manifest["commit"]:
        bad("provenance commit differs from release-manifest.json commit")
    if prov["predicate"]["buildDefinition"]["externalParameters"]["version"] != manifest["version"]:
        bad("provenance version differs from release-manifest.json version")
    builder = prov["predicate"]["runDetails"]["builder"]["builderDependencies"]
    if not builder or not builder[0].get("digest", {}).get("sha256"):
        bad("provenance has no builder image digest")

    # SBOMs are CycloneDX and describe the cog they sit under.
    for e in manifest["sboms"]:
        try:
            doc = json.loads((repo / e["path"]).read_text())
            comp = doc["metadata"]["component"]["name"]
            if doc["bomFormat"] != "CycloneDX" or comp != f"cog-{e['cog']}":
                bad(f"{e['path']}: not a CycloneDX SBOM for cog-{e['cog']} (component {comp!r})")
        except (OSError, ValueError, KeyError) as ex:
            bad(f"{e['path']}: {ex}")

    # Nothing unaccounted for in the tree.
    for p in sorted(repo.rglob("*")):
        rel = str(p.relative_to(repo))
        if p.is_file() and rel not in listed and not rel.endswith(".sig"):
            bad(f"{rel}: not listed in release-manifest.json")

    if manifest["dirty"] and not a.allow_dirty:
        bad("release was built from a dirty tree (pass --allow-dirty to accept)")

    if problems:
        print(f"release-verify: FAILED ({len(problems)})", file=sys.stderr)
        for m in problems:
            print(f"  {m}", file=sys.stderr)
        return 1
    print(f"release-verify: {manifest['version']} {manifest['commit'][:12]} OK: "
          f"{len(manifest['artifacts'])} artifacts, {len(manifest['sboms'])} SBOMs, "
          f"manifest and provenance signed by a pinned key")
    return 0


if __name__ == "__main__":
    sys.exit(main())
