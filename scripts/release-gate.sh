#!/usr/bin/env bash
# Controlled-host release check for COG-008. Does not generate a key, tag, or deploy.
#
#   scripts/release-gate.sh --key /secure/weavelogic-cog-release.pem --version v0.1.2 [--allow-dirty]
#
# The key file must already exist outside this repository. sign refuses a key
# whose public half is not the compiled WEAVELOGIC_PUBKEY_HEX.
#
#   scripts/cross-build.sh
#   weft-cog-repo sign --from .cargo-target/dist --out release/repo --key <pem>
#   weft-cog-repo verify release/repo
#   weft-cog-repo check-matrix --from .cargo-target/dist --cogs src/cogs release/repo
#
#   scripts/release-attest.py     CycloneDX SBOM per cog, in-toto/SLSA provenance, release-manifest.json
#   weft-cog-repo sign-file       detached .sig over release-manifest.json and provenance.intoto.json
#   scripts/release-verify.py     signatures, checksums, SBOMs, provenance, nothing unlisted
#
# The git tree must be clean before the build starts; --allow-dirty overrides that and the
# provenance and manifest then say the tree was dirty. --version is the tag being prepared
# (the gate does not create it).
#
# sign still returns success when at least one cog is signable. check-matrix
# fails unless every cog under src/cogs has arm and arm64 in registry.json,
# and x86_64 when that binary is in the dist directory.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
key=""
version=""
allow_dirty=()
from="$ROOT/.cargo-target/dist"
out="$ROOT/release/repo"
while [ $# -gt 0 ]; do
  case "$1" in
    --key) key="${2:?release-gate: --key needs a path}"; shift 2 ;;
    --from) from="${2:?release-gate: --from needs a directory}"; shift 2 ;;
    --version) version="${2:?release-gate: --version needs a tag like v0.1.2}"; shift 2 ;;
    --allow-dirty) allow_dirty=(--allow-dirty); shift ;;
    --out) out="${2:?release-gate: --out needs a directory}"; shift 2 ;;
    -h|--help) sed -n '2,26p' "$0"; exit 0 ;;
    *) echo "release-gate: unknown argument $1" >&2; exit 2 ;;
  esac
done
if [ -z "$key" ]; then
  echo "release-gate: pass --key <pem>. This script does not generate a key." >&2
  exit 2
fi
if [ -z "$version" ]; then
  echo "release-gate: pass --version <tag>, for example v0.1.2" >&2
  exit 2
fi
if [ ${#allow_dirty[@]} -eq 0 ] && [ -n "$(git status --porcelain)" ]; then
  echo "release-gate: git tree is dirty; commit first or pass --allow-dirty" >&2
  exit 1
fi
if [ ! -f "$key" ]; then
  echo "release-gate: key file not found" >&2
  exit 1
fi
key_dir="$(cd "$(dirname "$key")" && pwd)"
key_abs="$key_dir/$(basename "$key")"
case "$key_abs" in
  "$ROOT"|"$ROOT"/*)
    echo "release-gate: the key must stay outside the repository" >&2
    exit 2
    ;;
esac

export CARGO_TARGET_DIR="$ROOT/.cargo-target/host"
bin="$CARGO_TARGET_DIR/debug/weft-cog-repo"
if [ ! -x "$bin" ]; then
  cargo build --locked -p cog-repo --bin weft-cog-repo
fi

build_started="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
scripts/cross-build.sh
build_finished="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
"$bin" sign --from "$from" --out "$out" --key "$key"
"$bin" verify "$out"
"$bin" check-matrix --from "$from" --cogs "$ROOT/src/cogs" "$out"
echo "release-gate: matrix matches registry.json"

python3 scripts/release-attest.py --repo "$out" --version "$version" \
  --image "$(scripts/cross-build.sh --print-image)" \
  --build-started "$build_started" --build-finished "$build_finished" \
  ${allow_dirty[@]+"${allow_dirty[@]}"}
for doc in release-manifest.json provenance.intoto.json; do
  "$bin" sign-file "$out/$doc" --key "$key"
done
python3 scripts/release-verify.py "$out" --bin "$bin" ${allow_dirty[@]+"${allow_dirty[@]}"}

echo
echo "release-gate: summary for $version"
echo "  cogs signed and matrix checked: $(python3 -c 'import json,sys;print(len(json.load(open(sys.argv[1]))["cogs"]))' "$out/registry.json")"
echo "  release manifest: $out/release-manifest.json (signed)"
echo "  provenance:       $out/provenance.intoto.json (signed)"
echo "  SBOMs:            $(ls "$out"/*/sbom.cdx.json | wc -l | tr -d ' ')"
echo "release-gate: PASSED"
