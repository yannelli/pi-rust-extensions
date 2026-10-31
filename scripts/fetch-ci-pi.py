#!/usr/bin/env python3
"""Fetch the published pi host for CI, verify it against pinned hashes and the release, and export PI_BIN."""

import argparse
import hashlib
import json
import os
import platform
import subprocess
import sys
import tarfile
import urllib.request
from pathlib import Path

REPO = "yannelli/pi-rust"
TAG = "v0.9.2"
VERSION = TAG[1:]
API = f"https://api.github.com/repos/{REPO}/releases/tags/{TAG}"
DOWNLOAD = f"https://github.com/{REPO}/releases/download/{TAG}"
# SHA-256 of the published v0.9.2 assets, recorded when the release was verified.
PINNED = {
    "SHA256SUMS": "7f89d0424cfcdc83e08b910ff48a3da38029319fba8304214614b6d88ad178ec",
    "pi-v0.9.2-aarch64-apple-darwin.tar.gz": "6ff2f6e92e1c604b65e40e1dc4ce1820b0a25795cb3367644be5758fa81c988f",
    "pi-v0.9.2-x86_64-apple-darwin.tar.gz": "dec4cb1c6f6ce5b14c949bcb06acb760aa1179bcde43e5b8dda7fd853d418d87",
    "pi-v0.9.2-x86_64-unknown-linux-musl.tar.gz": "0b6c759990be12f1859651121f749bf2f51f35e9ca43ad62ba9d1b9da18c981a",
    "pi-v0.9.2-aarch64-unknown-linux-musl.tar.gz": "5c3d5d708b2d3dd5e899ab49a828d9376904b22f0c3e9af1875bf83914f11967",
}
TARGETS = {
    ("Darwin", "arm64"): "aarch64-apple-darwin",
    ("Darwin", "x86_64"): "x86_64-apple-darwin",
    ("Linux", "x86_64"): "x86_64-unknown-linux-musl",
    ("Linux", "aarch64"): "aarch64-unknown-linux-musl",
}
MAX_BYTES = 128 << 20


def fail(message):
    print(f"fetch-ci-pi: {message}", file=sys.stderr)
    sys.exit(1)


def request(url, accept):
    headers = {"Accept": accept, "User-Agent": "pi-rust-extensions-ci"}
    token = os.environ.get("GH_TOKEN")
    if token and url.startswith("https://api.github.com/"):
        headers["Authorization"] = f"Bearer {token}"
    return urllib.request.Request(url, headers=headers)


def fetch(url, accept, limit, timeout=120):
    with urllib.request.urlopen(request(url, accept), timeout=timeout) as response:
        if not response.geturl().startswith("https://"):
            fail(f"{url} redirected off HTTPS")
        data = response.read(limit + 1)
    if len(data) > limit:
        fail(f"{url} is larger than {limit} bytes")
    return data


def release_assets():
    release = json.loads(fetch(API, "application/vnd.github+json", 4 << 20, timeout=60))
    if release.get("tag_name") != TAG or release.get("draft") or release.get("prerelease"):
        fail(f"release {TAG} is not a published, non-prerelease release")
    return {asset["name"]: asset for asset in release.get("assets", [])}


def download(name, assets):
    asset = assets.get(name)
    if not asset:
        fail(f"release {TAG} has no asset {name}")
    if asset.get("digest") != f"sha256:{PINNED[name]}":
        fail(f"{name}: API digest {asset.get('digest')} is not the pinned sha256")
    data = fetch(f"{DOWNLOAD}/{name}", "application/octet-stream", min(asset["size"], MAX_BYTES))
    digest = hashlib.sha256(data).hexdigest()
    if len(data) != asset["size"] or digest != PINNED[name]:
        fail(f"{name}: downloaded {len(data)} bytes with sha256 {digest}")
    return data


def checksum_of(sums, name):
    for line in sums.decode().splitlines():
        parts = line.split()
        if len(parts) == 2 and parts[1].lstrip("*") == name:
            return parts[0]
    fail(f"SHA256SUMS has no line for {name}")


def extract(archive_path, target, dest):
    member_name = f"pi-{TAG}-{target}/pi"
    with tarfile.open(archive_path, "r:gz") as archive:
        matches = [member for member in archive.getmembers() if member.name == member_name]
        if len(matches) != 1 or not matches[0].isreg() or matches[0].size > MAX_BYTES:
            fail(f"{archive_path.name} must hold exactly one regular file {member_name}")
        data = archive.extractfile(matches[0]).read()
    binary = dest / "pi"
    descriptor = os.open(binary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o755)
    with os.fdopen(descriptor, "wb") as handle:
        handle.write(data)
    os.chmod(binary, 0o755)
    return binary, hashlib.sha256(data).hexdigest()


def check_version(binary, dest):
    home = dest / "version-home"
    home.mkdir()
    env = {"PATH": "/usr/bin:/bin", "HOME": str(home), "PI_CODING_AGENT_DIR": str(home / "agent"),
           "PI_OFFLINE": "1", "PI_SKIP_VERSION_CHECK": "1", "LANG": "C"}
    done = subprocess.run([str(binary), "--version"], env=env, capture_output=True, timeout=60, check=False)
    output = (done.stdout + done.stderr).decode("utf-8", "replace").strip()
    if done.returncode != 0 or VERSION not in output.split():
        fail(f"{binary} --version exited {done.returncode}: {output[:200]}")
    return output


def main():
    parser = argparse.ArgumentParser(description=f"Fetch and verify the published pi {TAG} host.")
    parser.add_argument("--dest", required=True, help="new directory for the host binary and evidence")
    parser.add_argument("--github-env", default=os.environ.get("GITHUB_ENV"), help="file to append PI_BIN to")
    args = parser.parse_args()
    target = TARGETS.get((platform.system(), platform.machine()))
    if not target:
        fail(f"no published pi host for {platform.system()} {platform.machine()}")
    dest = Path(args.dest).resolve()
    dest.mkdir(parents=True, exist_ok=False)
    name = f"pi-{TAG}-{target}.tar.gz"
    assets = release_assets()
    sums = download("SHA256SUMS", assets)
    if checksum_of(sums, name) != PINNED[name]:
        fail(f"SHA256SUMS lists another sha256 for {name}")
    archive_path = dest / name
    archive_path.write_bytes(download(name, assets))
    binary, binary_sha = extract(archive_path, target, dest)
    version = check_version(binary, dest)
    evidence = {"repo": REPO, "tag": TAG, "target": target, "archive": name, "archiveSha256": PINNED[name],
                "binary": str(binary), "binarySha256": binary_sha, "version": version}
    (dest / "host.json").write_text(json.dumps(evidence, indent=2) + "\n")
    if args.github_env:
        with open(args.github_env, "a") as env_file:
            env_file.write(f"PI_BIN={binary}\nPI_HOST_SHA256={binary_sha}\nPI_HOST_TAG={TAG}\n")
    print(json.dumps(evidence, indent=2))


if __name__ == "__main__":
    main()
