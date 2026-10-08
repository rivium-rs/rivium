#!/usr/bin/env bash
# Hosting smoke under systemd (run by hosting/systemd; also runs locally with Docker). The smoke
# binary runs as a service whose unit matches the deployment's (Type=simple, Restart=always, no
# RestartSec, TimeoutStopSec or KillSignal) inside a systemd container. Asserts: a run that exits
# with 75 is restarted; `systemctl stop` stops the service gracefully (SIGTERM logged, exit status
# 0, under 4.5 s, no SIGKILL).
# Usage: scripts/hosting/systemd.sh <smoke binary for the Docker host; its glibc must not exceed
#        the base image's (RIVIUM_SYSTEMD_BASE, default ubuntu:24.04)> [extra serve arguments]
# Checks are passed as strings and evaluated later, hence single quotes.
# shellcheck disable=SC2016
set -euo pipefail
bin=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
shift
extra=${*:-}
base=${RIVIUM_SYSTEMD_BASE:-ubuntu:24.04}
image=rivium-systemd-smoke
name=rivium-systemd-smoke-$$
failures=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; failures=$((failures + 1)); fi; }

docker build -q -t "$image" - > /dev/null <<EOF
FROM $base
RUN apt-get update && apt-get install -y --no-install-recommends systemd && rm -rf /var/lib/apt/lists/*
CMD ["/lib/systemd/systemd"]
EOF
docker run -d --name "$name" --privileged --cgroupns=private --tmpfs /run --tmpfs /run/lock \
  -v "$bin":/opt/smoke/smoke:ro "$image" > /dev/null
trap 'docker rm -f "$name" > /dev/null' EXIT
ct() { docker exec -i "$name" "$@"; }

state=unknown
for _ in $(seq 60); do
  state=$(ct systemctl is-system-running 2> /dev/null || true)
  case "$state" in running | degraded) break ;; esac
  sleep 1
done
echo "systemd in container: $state"
# Debug level makes systemd log each main-process exit status (they are reset once a unit stops).
ct systemd-analyze log-level debug

ct sh -c 'mkdir -p /var/lib/smoke && cat > /etc/systemd/system/smoke.service' <<EOF
[Unit]
Description=Rivium hosting smoke

[Service]
Type=simple
ExecStart=/opt/smoke/smoke serve --log /var/lib/smoke/smoke.log --exit-once 75 --marker /var/lib/smoke/exited $extra
Restart=always
EOF
ct systemctl daemon-reload
ct systemctl start smoke
# The first run exits with 75; systemd restarts it (RestartSec defaults to 100 ms).
for _ in $(seq 150); do
  grep -q ' ready$' <<< "$(ct cat /var/lib/smoke/smoke.log 2> /dev/null)" && break
  sleep 0.2
done
# grep reads here-strings, not pipes: with pipefail, `journalctl | grep -q` fails on SIGPIPE.
journal() { ct journalctl -o cat; }
check "first run exited with 75" 'grep -q "smoke.service: Main process exited, code=exited, status=75/" <<< "$(journal)"'
check "exit 75 is restarted (NRestarts >= 1)" '[ "$(ct systemctl show -p NRestarts --value smoke)" -ge 1 ]'
check "restarted run is ready" 'grep -q " ready$" <<< "$(ct cat /var/lib/smoke/smoke.log)"'

elapsed_ms=$(ct sh -c 's=$(date +%s%N); systemctl stop smoke; e=$(date +%s%N); echo $(( (e - s) / 1000000 ))')
echo "systemctl stop took ${elapsed_ms} ms"
check "stop finished in < 4500 ms" '[ "$elapsed_ms" -lt 4500 ]'
check "stopped run exited with status 0" \
  'grep "smoke.service: Main process exited" <<< "$(journal)" | tail -n 1 | grep -c "code=exited, status=0/SUCCESS" > /dev/null'
check "SIGTERM and graceful stop logged" 'ct grep -q "stop signal=SIGTERM" /var/lib/smoke/smoke.log && ct grep -q " stopped$" /var/lib/smoke/smoke.log'
check "no SIGKILL" '! grep "smoke" <<< "$(journal)" | grep -iE "sigkill|code=killed"'
echo "--- systemd records"
journal | grep -E "smoke.service: (Main process exited|Deactivated|Scheduled restart)" || true
echo "--- smoke log"
ct cat /var/lib/smoke/smoke.log || true
[ "$failures" -eq 0 ] || { echo "hosting/systemd: $failures failure(s)"; exit 1; }
echo "hosting/systemd: passed"
