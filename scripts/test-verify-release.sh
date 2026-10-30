#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
verify="$root/scripts/verify-release.sh"
timeout_command="$(command -v timeout || command -v gtimeout || true)"
if [ -z "$timeout_command" ]; then
  command -v perl >/dev/null || { echo "test-verify-release: timeout, gtimeout, or perl is necessary" >&2; exit 1; }
  perl_timeout() { perl -e 'alarm shift; exec @ARGV or die "exec: $!"' "$@"; }
  timeout_command=perl_timeout
fi

if ! openssl version 2> /dev/null | grep -q '^OpenSSL 3'; then
  for prefix in /opt/homebrew/opt/openssl@3 /usr/local/opt/openssl@3; do
    if [ -x "$prefix/bin/openssl" ]; then
      PATH="$prefix/bin:$PATH"
      break
    fi
  done
fi
openssl version | grep -q '^OpenSSL 3' || { echo "test-verify-release: OpenSSL 3 is necessary" >&2; exit 1; }

version="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' "$root/Cargo.toml")"
tag="v$version"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

release="$work/release"
mkdir "$release"
targets="aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-musl aarch64-unknown-linux-musl x86_64-pc-windows-msvc"
for target in $targets; do
  case "$target" in
    *-windows-*) name="pi-rust-ext-$target.zip" ;;
    *) name="pi-rust-ext-$target.tar.gz" ;;
  esac
  printf 'placeholder archive for %s\n' "$target" > "$release/$name"
done
(cd "$release" && shasum -a 256 pi-rust-ext-* > SHA256SUMS)

(cd "$root" && "$timeout_command" 300 cargo run --quiet --offline --locked -p marketplace-index -- \
  "$root/marketplace.json" "$tag" "$release" "$release/index.json")

openssl genpkey -algorithm ed25519 -out "$work/key.pem" 2> /dev/null
public_key="$(openssl pkey -in "$work/key.pem" -pubout -outform DER | tail -c 32 | openssl base64 -A)"

sign() {
  openssl pkeyutl -sign -rawin -inkey "$work/key.pem" -in "$1/index.json" | openssl base64 -A > "$1/index.json.sig"
}
sign "$release"

failures=0
fresh() {
  local dir
  dir="$(mktemp -d "$work/case.XXXXXX")"
  cp "$release"/* "$dir"
  echo "$dir"
}
edit_index() {
  jq "$2" "$1/index.json" > "$1/index.tmp"
  mv "$1/index.tmp" "$1/index.json"
}
expect_pass() {
  local label="$1"
  shift
  if out="$("$timeout_command" 60 bash "$verify" "$@" 2>&1)"; then
    echo "pass: $label"
  else
    echo "FAIL: $label: expected success" >&2
    echo "$out" >&2
    failures=$((failures + 1))
  fi
}
expect_fail() {
  local label="$1" message="$2"
  shift 2
  if out="$("$timeout_command" 60 bash "$verify" "$@" 2>&1)"; then
    echo "FAIL: $label: expected rejection" >&2
    failures=$((failures + 1))
  elif ! grep -qF -- "$message" <<< "$out"; then
    echo "FAIL: $label: expected \"$message\"" >&2
    echo "$out" >&2
    failures=$((failures + 1))
  else
    echo "pass: $label"
  fi
}

expect_pass "valid release" --public-key "$public_key" "$tag" "$release"
expect_fail "default key rejects throwaway signature" "signature is not valid" "$tag" "$release"

dir="$(fresh)"
edit_index "$dir" '.minPiVersion = "1.2.3-rc.1"'
sign "$dir"
expect_pass "prerelease minPiVersion" --public-key "$public_key" "$tag" "$dir"

protocol_cases=('del(.protocolVersion)' '.protocolVersion = 0' '.protocolVersion = 2' '.protocolVersion = "1"')
for filter in "${protocol_cases[@]}"; do
  dir="$(fresh)"
  edit_index "$dir" "$filter"
  sign "$dir"
  expect_fail "protocolVersion: $filter" "protocolVersion is not the number 1" --public-key "$public_key" "$tag" "$dir"
done

min_pi_cases=(
  'del(.minPiVersion)'
  '.minPiVersion = null'
  '.minPiVersion = ""'
  '.minPiVersion = "1.2"'
  '.minPiVersion = 123'
  '.minPiVersion = "01.2.3"'
  '.minPiVersion = "1.2.3-01"'
  '.minPiVersion = "1.2.3+build"'
  '.minPiVersion = "1.2.3\n"'
  '.minPiVersion = "1.2.3\u0000"'
)
for filter in "${min_pi_cases[@]}"; do
  dir="$(fresh)"
  edit_index "$dir" "$filter"
  sign "$dir"
  expect_fail "minPiVersion: $filter" "minPiVersion is not a SemVer string" --public-key "$public_key" "$tag" "$dir"
done

dir="$(fresh)"
edit_index "$dir" '.minPiVersion = "9.9.9"'
expect_fail "index edited after signing" "signature is not valid" --public-key "$public_key" "$tag" "$dir"

dir="$(fresh)"
head -c 64 /dev/zero | openssl base64 -A > "$dir/index.json.sig"
expect_fail "replaced signature" "signature is not valid" --public-key "$public_key" "$tag" "$dir"

first="pi-rust-ext-aarch64-apple-darwin.tar.gz"
dir="$(fresh)"
printf 'placeholder archive for aarch64-apple-darwiX\n' > "$dir/$first"
expect_fail "archive content changed" "$first: SHA-256" --public-key "$public_key" "$tag" "$dir"

dir="$(fresh)"
edit_index "$dir" '.binaries["pi-rust-ext"]["aarch64-apple-darwin"].sha256 = ("0" * 64)'
sign "$dir"
expect_fail "re-signed wrong checksum" "$first: SHA-256" --public-key "$public_key" "$tag" "$dir"

dir="$(fresh)"
edit_index "$dir" '.binaries["pi-rust-ext"]["aarch64-apple-darwin"].size += 1'
sign "$dir"
expect_fail "re-signed wrong size" "$first: size" --public-key "$public_key" "$tag" "$dir"

if [ "$failures" -ne 0 ]; then
  echo "test-verify-release: $failures case(s) failed" >&2
  exit 1
fi
echo "test-verify-release: all cases passed"
