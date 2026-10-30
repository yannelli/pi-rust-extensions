#!/usr/bin/env bash
# Verifies a marketplace release: the Ed25519 signature of index.json, then the SHA-256 and size of each archive.
set -euo pipefail

REPOSITORY="yannelli/pi-rust-extensions"
PUBLIC_KEY="G33uzqR2bwGJyZD3qnY4BNThlZzwyntF+DWRTMQYBBo="
TARGETS="aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-musl aarch64-unknown-linux-musl x86_64-pc-windows-msvc"

die() {
  echo "verify-release: $*" >&2
  exit 1
}

usage() {
  echo "usage: scripts/verify-release.sh <tag> [<dir>]" >&2
  echo "With <dir>, checks index.json, index.json.sig, and the archives in <dir> and downloads nothing." >&2
  exit 2
}

[ $# -ge 1 ] && [ $# -le 2 ] || usage
tag="$1"
[[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || die "invalid tag: $tag"
for tool in openssl jq shasum curl; do
  command -v "$tool" > /dev/null || die "$tool is not installed"
done
openssl version | grep -q '^OpenSSL 3' || die "OpenSSL 3 is necessary for Ed25519 -rawin (found: $(openssl version))"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
base="https://github.com/$REPOSITORY/releases/download/$tag"
download() {
  curl -fsSL --proto '=https' --retry 3 -o "$2" "$1" || die "download failed: $1"
}

if [ $# -eq 2 ]; then
  dir="$2"
  [ -d "$dir" ] || die "not a directory: $dir"
else
  dir="$work/release"
  mkdir "$dir"
  download "$base/index.json" "$dir/index.json"
  download "$base/index.json.sig" "$dir/index.json.sig"
fi

# Ed25519 SubjectPublicKeyInfo DER: the RFC 8410 prefix, then the 32 raw key bytes.
{
  printf '\x30\x2a\x30\x05\x06\x03\x2b\x65\x70\x03\x21\x00'
  printf '%s' "$PUBLIC_KEY" | openssl base64 -d -A
} > "$work/public.der"
[ "$(wc -c < "$work/public.der" | tr -d ' ')" = 44 ] || die "invalid public key"
tr -d '\r\n' < "$dir/index.json.sig" | openssl base64 -d -A > "$work/index.sig.bin"
[ "$(wc -c < "$work/index.sig.bin" | tr -d ' ')" = 64 ] || die "index.json.sig is not a 64-byte Ed25519 signature"
openssl pkeyutl -verify -pubin -keyform DER -inkey "$work/public.der" -rawin \
  -in "$dir/index.json" -sigfile "$work/index.sig.bin" > /dev/null \
  || die "index.json signature is not valid"
echo "ok: index.json signature"

index="$dir/index.json"
[ "$(jq -r '.schemaVersion' "$index")" = 1 ] || die "schemaVersion is not 1"
[ "$(jq -r '.repository' "$index")" = "$REPOSITORY" ] || die "repository is not $REPOSITORY"
[ "$(jq -r '.tag' "$index")" = "$tag" ] || die "index tag is not $tag"
[ "$(jq -r '.binaries["pi-rust-ext"] | length' "$index")" = 5 ] || die "index does not list 5 targets"

for target in $TARGETS; do
  case "$target" in
    *-windows-*) name="pi-rust-ext-$target.zip" ;;
    *) name="pi-rust-ext-$target.tar.gz" ;;
  esac
  entry="$(jq -r --arg t "$target" '.binaries["pi-rust-ext"][$t] // empty | [.url, .sha256, .size] | @tsv' "$index")"
  [ -n "$entry" ] || die "index has no $target archive"
  IFS=$'\t' read -r url sha256 size <<< "$entry"
  [ "$url" = "$base/$name" ] || die "$target: unexpected URL $url"
  [[ "$sha256" =~ ^[0-9a-f]{64}$ ]] || die "$target: invalid sha256"
  [ $# -eq 2 ] || download "$url" "$dir/$name"
  [ -f "$dir/$name" ] || die "missing archive: $name"
  actual_sha="$(shasum -a 256 "$dir/$name" | cut -d' ' -f1)"
  actual_size="$(wc -c < "$dir/$name" | tr -d ' ')"
  [ "$actual_sha" = "$sha256" ] || die "$name: SHA-256 $actual_sha does not match index $sha256"
  [ "$actual_size" = "$size" ] || die "$name: size $actual_size does not match index $size"
  echo "ok: $name"
done
echo "verified $tag"
