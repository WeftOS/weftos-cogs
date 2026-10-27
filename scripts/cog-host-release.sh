#!/usr/bin/env bash
# Signable release of weft-cog-host (crates/cog-host) for the three Cog Host platforms.
#
#   scripts/cog-host-release.sh --version v0.1.3 [--out release/cog-host] [--allow-dirty]
#
# Targets and the hosts they are for:
#   x86_64-unknown-linux-gnu        x86-64 Linux servers (Ubuntu 24.04 and later)
#   aarch64-unknown-linux-gnu       arm64 boards (Ubuntu 24.04, Raspberry Pi OS bookworm)
#   armv7-unknown-linux-gnueabihf   Pi Zero 2 W class boards (Raspberry Pi OS bookworm)
#
# The build runs in the same container image as scripts/cross-build.sh
# (cross-build.sh --print-image; built from scripts/cross/Dockerfile when missing), with the
# toolchain from rust-toolchain.toml, `cargo build --locked --release`, and a cross strip.
# The image is Debian bookworm, so every binary links against glibc 2.36 at most; the script
# fails if `objdump -T` shows a higher GLIBC_ symbol version. No target-cpu tuning: the
# aarch64 binary runs on any ARMv8-A core, the x86_64 one on baseline x86-64.
#
# Output (<out>, default release/cog-host/, git-ignored):
#   weft-cog-host-<target>          stripped binaries
#   sbom.cdx.json                   CycloneDX 1.5 SBOM for the crate (cargo-cyclonedx, all targets)
#   provenance.intoto.json          in-toto v1 Statement, SLSA provenance v1, binaries + SBOM as subjects
#   cog-host-manifest.json          version, commit, per binary target/size/sha256/glibc/builder image digest
#   SHA256SUMS                      sha256sum format over everything above
#
# Signing is a separate, manual step on the controlled host (this script never reads a key):
#   weft-cog-repo sign-file <out>/cog-host-manifest.json --key <KEY>
#   weft-cog-repo sign-file <out>/provenance.intoto.json --key <KEY>
# Verify with scripts/release-verify.py --cog-host <out> (signatures, checksums, provenance).
#
# The git tree must be clean; --allow-dirty overrides that and the manifest and provenance say so.
# Needs docker and cargo-cyclonedx on the host.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
version=""
out="$ROOT/release/cog-host"
allow_dirty=()
while [ $# -gt 0 ]; do
  case "$1" in
    --version) version="${2:?cog-host-release: --version needs a tag like v0.1.3}"; shift 2 ;;
    --out) out="${2:?cog-host-release: --out needs a directory}"; shift 2 ;;
    --allow-dirty) allow_dirty=(--allow-dirty); shift ;;
    -h|--help) sed -n '2,33p' "$0"; exit 0 ;;
    *) echo "cog-host-release: unknown argument $1" >&2; exit 2 ;;
  esac
done
[ -n "$version" ] || { echo "cog-host-release: pass --version <tag>, for example v0.1.3" >&2; exit 2; }
if [ ${#allow_dirty[@]} -eq 0 ] && [ -n "$(git status --porcelain)" ]; then
  echo "cog-host-release: git tree is dirty; commit first or pass --allow-dirty" >&2
  exit 1
fi
command -v docker >/dev/null || { echo "cog-host-release: docker not found" >&2; exit 1; }

TARGETS=(x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu armv7-unknown-linux-gnueabihf)
IMAGE="$(scripts/cross-build.sh --print-image)"
plat_args=()
[ -n "${CROSS_PLATFORM:-}" ] && plat_args=(--platform "$CROSS_PLATFORM")
if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
  echo "cog-host-release: building image $IMAGE"
  docker build ${plat_args[@]+"${plat_args[@]}"} -t "$IMAGE" "$ROOT/scripts/cross"
fi

mkdir -p "$out"
out="$(cd "$out" && pwd)"
case "$out" in
  "$ROOT"/*) rel_out="${out#"$ROOT"/}" ;;
  *) echo "cog-host-release: --out must be inside the repository (the container mounts only it)" >&2; exit 2 ;;
esac
rm -f "$out"/weft-cog-host-* "$out"/*.json "$out"/*.sig "$out"/SHA256SUMS

build_started="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
# --remap-path-prefix keeps container paths out of panic messages and debug strings.
docker run --rm ${plat_args[@]+"${plat_args[@]}"} \
  -v "$ROOT":/work -w /work \
  -v weavelogic-cogs-cargo-registry:/usr/local/cargo/registry \
  -e CARGO_TARGET_DIR=/work/.cargo-target/cog-host-release \
  -e CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_LINKER=arm-linux-gnueabihf-gcc \
  -e CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
  -e CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
  -e RUSTFLAGS="--remap-path-prefix=/work=. --remap-path-prefix=/usr/local/cargo/registry=cargo-registry" \
  "$IMAGE" bash -euo pipefail -c '
    out="/work/$1"; shift
    rustup target add "$@"
    declare -A strip=([x86_64-unknown-linux-gnu]=x86_64-linux-gnu-strip
                      [aarch64-unknown-linux-gnu]=aarch64-linux-gnu-strip
                      [armv7-unknown-linux-gnueabihf]=arm-linux-gnueabihf-strip)
    declare -A objdump=([x86_64-unknown-linux-gnu]=x86_64-linux-gnu-objdump
                        [aarch64-unknown-linux-gnu]=aarch64-linux-gnu-objdump
                        [armv7-unknown-linux-gnueabihf]=arm-linux-gnueabihf-objdump)
    for t in "$@"; do
      echo "== weft-cog-host $t"
      cargo build --locked --release -p cog-host --bin weft-cog-host --target "$t"
      # --sparse=never: see scripts/cross-build.sh (bind-mounted APFS refuses hole punching).
      cp --sparse=never "/work/.cargo-target/cog-host-release/$t/release/weft-cog-host" "$out/weft-cog-host-$t"
      "${strip[$t]}" "$out/weft-cog-host-$t"
      "${objdump[$t]}" -T "$out/weft-cog-host-$t" | grep -o "GLIBC_[0-9.]*" | sort -uV > "$out/.glibc-$t"
    done
    rustc --version > "$out/.rustc"
  ' _ "$rel_out" "${TARGETS[@]}"
build_finished="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

python3 "$ROOT/scripts/cog-host-attest.py" --out "$out" --version "$version" --image "$IMAGE" \
  --build-started "$build_started" --build-finished "$build_finished" \
  ${allow_dirty[@]+"${allow_dirty[@]}"} "${TARGETS[@]}"
