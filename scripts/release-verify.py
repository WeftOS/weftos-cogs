#!/usr/bin/env python3
"""Verify a cogs release directory: signatures, checksums, SBOMs, provenance.

  scripts/release-verify.py <repo-dir> [--bin weft-cog-repo] [--allow-dirty] [--pin <pubkey-hex>]...
  scripts/release-verify.py --cog-host <dir> [--bin weft-cog-repo] [--allow-dirty] [--pin <pubkey-hex>]...

<repo-dir> holds registry.json, release-manifest.json(.sig), provenance.intoto.json(.sig) and
the artifact tree, as downloaded from a release. Signatures are checked with
`weft-cog-repo verify-file` against the compiled-in WeaveLogic key (or each --pin).
Per-binary signatures are the job of `weft-cog-repo verify <repo-dir>`; run that too.
With --cog-host, <dir> is a scripts/cog-host-release.sh output instead: cog-host-manifest.json(.sig),
provenance.intoto.json(.sig), the weft-cog-host-<target> binaries, sbom.cdx.json and SHA256SUMS.
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


def check_signatures(a, repo, names, bad):
    for name in names:
        cmd = [a.bin, "verify-file", str(repo / name)]
        for k in a.pin:
            cmd += ["--pin", k]
        r = subprocess.run(cmd, capture_output=True, text=True)
        if r.returncode != 0:
            bad(f"{name}: signature: {(r.stderr or r.stdout).strip()}")


def report(problems, ok):
    if problems:
        print(f"release-verify: FAILED ({len(problems)})", file=sys.stderr)
        for m in problems:
            print(f"  {m}", file=sys.stderr)
        return 1
    print(ok)
    return 0


def verify_cog_host(a, repo):
    problems = []
    bad = problems.append
    check_signatures(a, repo, ("cog-host-manifest.json", "provenance.intoto.json"), bad)
    try:
        manifest = json.loads((repo / "cog-host-manifest.json").read_text())
        prov = json.loads((repo / "provenance.intoto.json").read_text())
    except (OSError, ValueError) as e:
        print(f"release-verify: FAILED: {e}", file=sys.stderr)
        return 1
    if manifest.get("kind") != "weavelogic-cog-host-release":
        bad(f"cog-host-manifest.json: kind is {manifest.get('kind')!r}")

    listed = {"cog-host-manifest.json", "provenance.intoto.json", "SHA256SUMS"}
    for e in manifest["binaries"] + [manifest["sbom"], manifest["provenance"]]:
        p = repo / e["path"]
        listed.add(e["path"])
        if not p.is_file():
            bad(f"{e['path']}: missing")
        elif sha256_file(p) != e["sha256"] or p.stat().st_size != e["size"]:
            bad(f"{e['path']}: sha256 or size differs from cog-host-manifest.json")
    for b in manifest["binaries"]:
        if b["path"] != f"weft-cog-host-{b['target']}":
            bad(f"{b['path']}: name does not match target {b['target']}")
        if not b.get("builder", {}).get("digest", {}).get("sha256"):
            bad(f"{b['path']}: no builder image digest")

    want = {e["path"]: e["sha256"] for e in manifest["binaries"] + [manifest["sbom"]]}
    got = {s["name"]: s["digest"]["sha256"] for s in prov["subject"]}
    if want != got:
        bad("provenance subjects differ from cog-host-manifest.json binaries + SBOM")
    ext = prov["predicate"]["buildDefinition"]["externalParameters"]
    if ext["source"]["digest"]["gitCommit"] != manifest["commit"]:
        bad("provenance commit differs from cog-host-manifest.json commit")
    if ext["version"] != manifest["version"]:
        bad("provenance version differs from cog-host-manifest.json version")

    try:
        sums = dict(reversed(line.split("  ", 1))
                    for line in (repo / "SHA256SUMS").read_text().splitlines() if line)
        for path, sha in sums.items():
            if not (repo / path).is_file() or sha256_file(repo / path) != sha:
                bad(f"SHA256SUMS: {path} does not match")
        for path in listed - {"SHA256SUMS"}:
            if path not in sums:
                bad(f"SHA256SUMS: {path} not listed")
    except (OSError, ValueError) as e:
        bad(f"SHA256SUMS: {e}")
    try:
        doc = json.loads((repo / manifest["sbom"]["path"]).read_text())
        if doc["bomFormat"] != "CycloneDX" or doc["metadata"]["component"]["name"] != "cog-host":
            bad("sbom.cdx.json: not a CycloneDX SBOM for cog-host")
    except (OSError, ValueError, KeyError) as ex:
        bad(f"sbom.cdx.json: {ex}")

    for p in sorted(repo.rglob("*")):
        rel = str(p.relative_to(repo))
        if p.is_file() and rel not in listed and not rel.endswith(".sig"):
            bad(f"{rel}: not listed in cog-host-manifest.json")
    if manifest["dirty"] and not a.allow_dirty:
        bad("release was built from a dirty tree (pass --allow-dirty to accept)")
    return report(problems, f"release-verify: cog-host {manifest['version']} {manifest['commit'][:12]} OK: "
                            f"{len(manifest['binaries'])} binaries, SBOM, SHA256SUMS, "
                            f"manifest and provenance signed by a pinned key")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("repo")
    ap.add_argument("--bin", default="weft-cog-repo")
    ap.add_argument("--allow-dirty", action="store_true")
    ap.add_argument("--pin", action="append", default=[])
    ap.add_argument("--cog-host", action="store_true",
                    help="<repo> is a scripts/cog-host-release.sh output directory")
    a = ap.parse_args()
    repo = Path(a.repo)
    if a.cog_host:
        return verify_cog_host(a, repo)
    problems = []

    def bad(msg):
        problems.append(msg)

    check_signatures(a, repo, ("release-manifest.json", "provenance.intoto.json"), bad)

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

    return report(problems, f"release-verify: {manifest['version']} {manifest['commit'][:12]} OK: "
                            f"{len(manifest['artifacts'])} artifacts, {len(manifest['sboms'])} SBOMs, "
                            f"manifest and provenance signed by a pinned key")


if __name__ == "__main__":
    sys.exit(main())
