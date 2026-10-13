#!/usr/bin/env bash
# Read-only registry checks. Never retry a publication to resolve an uncertain HTTP result.
set -euo pipefail
if [ "$#" -lt 2 ] || [ "$#" -gt 3 ]; then
  echo 'usage: release-registry.sh <absent|published> <version> [package-dir]' >&2
  exit 2
fi
mode=$1; version=$2
[[ "$version" =~ ^(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})$ ]] || { echo 'release: invalid version' >&2; exit 1; }
[[ "$mode" = absent && "$#" -eq 2 || "$mode" = published && "$#" -eq 3 ]] || exit 2
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
for crate in rivium-error rivium rivium-http rivium-jni rivium-test; do
  # --fail cannot distinguish an absent version from a registry outage; handle every status explicitly.
  attempts=1
  [ "$mode" != published ] || attempts=6
  for ((attempt=1; attempt<=attempts; attempt++)); do
    status=$(curl --proto '=https' --tlsv1.2 --connect-timeout 10 --max-time 30 --silent --show-error \
      --user-agent 'rivium-release-check' --output "$work/response" --write-out '%{http_code}' \
      "https://crates.io/api/v1/crates/$crate/$version")
    if [ "$mode:$status" = published:404 ] && [ "$attempt" -lt "$attempts" ]; then sleep 5; else break; fi
  done
  if [ "$mode" = absent ]; then
    [ "$status" = 404 ] || { echo "release: $crate $version is not absent (HTTP $status)" >&2; exit 1; }
  else
    [ "$status" = 200 ] || { echo "release: $crate $version is unconfirmed (HTTP $status); do not republish blindly" >&2; exit 1; }
    expected=$(jq -er --arg name "$crate" --arg version "$version" '.version |
      select(.crate == $name and .num == $version and .yanked == false) |
      .checksum | select(type == "string" and test("^[a-f0-9]{64}$"))' "$work/response")
    file="$3/$crate-$version.crate"
    [ -s "$file" ] || { echo "release: missing local package: $crate" >&2; exit 1; }
    actual=$(shasum -a 256 "$file" | cut -d ' ' -f 1)
    [ "$actual" = "$expected" ] || { echo "release: registry checksum differs for $crate" >&2; exit 1; }
  fi
  echo "release: $crate $version $mode"
done
