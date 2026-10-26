#!/usr/bin/env python3
"""Release attestations for the cogs repository (COG-008 release gate).

Runs after `weft-cog-repo sign` has written <repo>/registry.json. Writes, inside <repo>:

  <cog>/sbom.cdx.json        CycloneDX 1.5 SBOM per cog, from its Cargo manifest (cargo-cyclonedx)
  provenance.intoto.json     in-toto v1 Statement, SLSA provenance v1
  release-manifest.json      release version, commit, sha256 of everything above

Signing is separate: release-gate.sh signs the last two with `weft-cog-repo sign-file`.

  scripts/release-attest.py --repo release/repo --version v0.1.2 \\
      --image <builder image> --build-started <iso> [--build-finished <iso>] [--allow-dirty]

Refuses a dirty git tree unless --allow-dirty is passed, in which case the provenance and the
manifest both say so. Paths in the output are relative to <repo>; no host paths are written.
"""
import argparse
import hashlib
import json
import re
import shutil
import subprocess
import sys
import uuid
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
STATEMENT = "https://in-toto.io/Statement/v1"
PREDICATE = "https://slsa.dev/provenance/v1"
TARGETS = ["armv7-unknown-linux-gnueabihf", "aarch64-unknown-linux-gnu"]


def die(msg):
    print(f"release-attest: {msg}", file=sys.stderr)
    sys.exit(1)


def run(cmd, cwd=ROOT):
    r = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True)
    if r.returncode != 0:
        die(f"{' '.join(cmd[:3])} failed: {r.stderr.strip() or r.stdout.strip()}")
    return r.stdout


def sha256_file(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def write_json(p, doc):
    p.write_text(json.dumps(doc, indent=2, sort_keys=True) + "\n")


def now():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def source_url():
    url = run(["git", "remote", "get-url", "origin"]).strip()
    url = re.sub(r"^(https?://)[^/@]+@", r"\1", url)  # never keep credentials
    m = re.match(r"^git@([^:]+):(.+)$", url)
    if m:
        url = f"https://{m.group(1)}/{m.group(2)}"
    return re.sub(r"\.git$", "", url)


def tree_is_dirty():
    return bool(run(["git", "status", "--porcelain"]).strip())


def image_digest(image):
    r = subprocess.run(
        ["docker", "image", "inspect", "--format", "{{.Id}}", image],
        capture_output=True, text=True,
    )
    if r.returncode != 0:
        die(f"docker image inspect {image} failed: {r.stderr.strip()}")
    digest = r.stdout.strip().removeprefix("sha256:")
    if not re.fullmatch(r"[0-9a-f]{64}", digest):
        die(f"unexpected image id {digest!r}")
    return digest


def make_sbom(cog_id, repo):
    cog_dir = ROOT / "src" / "cogs" / cog_id
    run(["cargo", "cyclonedx", "--format", "json", "--spec-version", "1.5",
         "--target", "all", "--override-filename", "sbom.cdx", "--quiet"], cwd=cog_dir)
    produced = cog_dir / "sbom.cdx.json"
    if not produced.is_file():
        die(f"cargo-cyclonedx wrote no SBOM for {cog_id}")
    dest = repo / cog_id / "sbom.cdx.json"
    dest.parent.mkdir(parents=True, exist_ok=True)
    shutil.move(str(produced), dest)
    doc = json.loads(dest.read_text())
    if doc.get("bomFormat") != "CycloneDX" or doc.get("specVersion") != "1.5":
        die(f"{cog_id}: SBOM is not CycloneDX 1.5")
    return dest


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--repo", required=True)
    ap.add_argument("--version", required=True, help="release tag, e.g. v0.1.2")
    ap.add_argument("--image", required=True, help="cross-build container image name")
    ap.add_argument("--build-started", required=True)
    ap.add_argument("--build-finished", default=None)
    ap.add_argument("--allow-dirty", action="store_true")
    a = ap.parse_args()

    if not re.fullmatch(r"v\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?", a.version):
        die(f"--version {a.version!r} is not a tag like v0.1.2")
    repo = Path(a.repo).resolve()
    registry_path = repo / "registry.json"
    if not registry_path.is_file():
        die("registry.json missing; run weft-cog-repo sign first")
    registry = json.loads(registry_path.read_text())

    dirty = tree_is_dirty()
    if dirty and not a.allow_dirty:
        die("git tree is dirty; commit or stash, or pass --allow-dirty (the output is then marked)")
    commit = run(["git", "rev-parse", "HEAD"]).strip()
    url = source_url()
    cyclonedx = run(["cargo", "cyclonedx", "--version"]).strip()

    # Stale attestations from an earlier run must not survive into this one.
    for old in repo.glob("*/sbom.cdx.json"):
        old.unlink()
    for name in ("provenance.intoto.json", "release-manifest.json"):
        for p in (repo / name, repo / f"{name}.sig"):
            p.unlink(missing_ok=True)

    sboms = {}
    for cog in registry["cogs"]:
        sboms[cog["id"]] = make_sbom(cog["id"], repo)
    if not a.allow_dirty and tree_is_dirty():
        die("SBOM generation changed the git tree (Cargo.lock out of date?)")

    artifacts = []
    for cog in registry["cogs"]:
        for arch, art in sorted(cog["artifacts"].items()):
            p = repo / art["path"]
            sha = sha256_file(p)
            if sha != art["sha256"]:
                die(f"{art['path']}: sha256 differs from registry.json")
            artifacts.append({"cog": cog["id"], "arch": arch, "path": art["path"],
                              "sha256": sha, "size": p.stat().st_size})
    sbom_entries = [{"cog": cid, "path": str(p.relative_to(repo)),
                     "sha256": sha256_file(p), "size": p.stat().st_size}
                    for cid, p in sorted(sboms.items())]
    # Staged agent manifests ride along so nothing in the tree is unaccounted for.
    manifest_entries = [{"path": str(p.relative_to(repo)), "sha256": sha256_file(p),
                         "size": p.stat().st_size}
                        for p in sorted(repo.glob("cogs/*/manifest-*.json"))]

    subjects = [{"name": e["path"], "digest": {"sha256": e["sha256"]}}
                for e in artifacts + sbom_entries]
    finished = a.build_finished or now()
    source_dep = {"uri": f"git+{url}@{commit}", "digest": {"gitCommit": commit}}
    if dirty:
        source_dep["annotations"] = {"dirty": True}
    provenance = {
        "_type": STATEMENT,
        "subject": subjects,
        "predicateType": PREDICATE,
        "predicate": {
            "buildDefinition": {
                "buildType": f"{url}/blob/{commit}/scripts/release-gate.sh",
                "externalParameters": {
                    "source": {"uri": f"git+{url}", "digest": {"gitCommit": commit}},
                    "version": a.version,
                    "entryPoint": "scripts/release-gate.sh",
                },
                "internalParameters": {
                    "allowDirty": a.allow_dirty,
                    "dirtyTree": dirty,
                    "targets": TARGETS,
                    "sbomTool": cyclonedx,
                },
                "resolvedDependencies": [source_dep],
            },
            "runDetails": {
                "builder": {
                    "id": f"{url}/blob/{commit}/scripts/release-gate.sh#controlled-host",
                    "builderDependencies": [{
                        "name": a.image,
                        "uri": f"docker-image:{a.image}",
                        "digest": {"sha256": image_digest(a.image)},
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
    prov_path = repo / "provenance.intoto.json"
    write_json(prov_path, provenance)

    manifest = {
        "schema": 1,
        "kind": "weavelogic-cogs-release",
        "version": a.version,
        "commit": commit,
        "source": url,
        "dirty": dirty,
        "created": now(),
        "registry": {"path": "registry.json", "sha256": sha256_file(registry_path)},
        "artifacts": artifacts,
        "manifests": manifest_entries,
        "sboms": sbom_entries,
        "provenance": {"path": prov_path.name, "sha256": sha256_file(prov_path)},
    }
    write_json(repo / "release-manifest.json", manifest)
    print(f"release-attest: {a.version} {commit[:12]}{' DIRTY' if dirty else ''}: "
          f"{len(artifacts)} artifacts, {len(sbom_entries)} SBOMs, provenance, manifest")


if __name__ == "__main__":
    main()
