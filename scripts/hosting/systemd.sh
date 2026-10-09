#!/usr/bin/env bash
# Hosting test under systemd (run by hosting/systemd; also runs locally with Docker). The udp-echo
# example runs as a service the way the deployment installs one: started without arguments from
# its directory, which is its root, next to a config.toml that only sets its address, under a
# unit like the deployment's (Type=simple, Restart=always, no RestartSec, TimeoutStopSec or
# KillSignal) inside a systemd container. Asserts: a restart request exits with 75 and systemd
# restarts the service; `systemctl stop` stops it gracefully (SIGTERM logged, exit status 0,
# under 4.5 s, no SIGKILL).
# Usage: scripts/hosting/systemd.sh <udp-echo binary for the Docker host; its glibc must not exceed
#        the base image's (RIVIUM_SYSTEMD_BASE, default ubuntu:24.04)> [extra run arguments]
# Checks are passed as strings and evaluated later, hence single quotes.
# shellcheck disable=SC2016
set -euo pipefail
bin=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
shift
extra=${*:-}
base=${RIVIUM_SYSTEMD_BASE:-ubuntu:24.04}
image=rivium-systemd-hosting
name=rivium-systemd-hosting-$$
dir=/opt/udp-echo
log=$dir/logs/udp-echo/udp-echo.log
port=7357
failures=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; failures=$((failures + 1)); fi; }

docker build -q -t "$image" - > /dev/null <<EOF
FROM $base
RUN apt-get update && apt-get install -y --no-install-recommends systemd && rm -rf /var/lib/apt/lists/*
CMD ["/lib/systemd/systemd"]
EOF
docker run -d --name "$name" --privileged --cgroupns=private --tmpfs /run --tmpfs /run/lock \
  -v "$bin":$dir/udp-echo:ro "$image" > /dev/null
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

printf '[echo]\naddr = "127.0.0.1:%s"\n' "$port" | ct sh -c "cat > $dir/config.toml"
ct sh -c 'cat > /etc/systemd/system/udp-echo.service' <<EOF
[Unit]
Description=Rivium hosting test

[Service]
Type=simple
ExecStart=$dir/udp-echo $extra
Restart=always
EOF
ct systemctl daemon-reload
ct systemctl start udp-echo
# grep reads here-strings, not pipes: with pipefail, `journalctl | grep -q` fails on SIGPIPE.
logged() { ct cat "$log" 2> /dev/null || true; }
runs() { grep -c 'phase changed phase="running"' <<< "$(logged)" || true; }
wait_runs() { # <count>: waits up to 30 s for that many runs to be running
  for _ in $(seq 150); do
    [ "$(runs)" -ge "$1" ] && return 0
    sleep 0.2
  done
  return 1
}
journal() { ct journalctl -o cat; }

check "first run is running" 'wait_runs 1'
ct bash -c "printf restart > /dev/udp/127.0.0.1/$port"
# The run exits with 75; systemd restarts it (RestartSec defaults to 100 ms).
check "restarted run is running" 'wait_runs 2'
check "the restart request exited with 75" 'grep -q "udp-echo.service: Main process exited, code=exited, status=75/" <<< "$(journal)"'
check "exit 75 is restarted (NRestarts >= 1)" '[ "$(ct systemctl show -p NRestarts --value udp-echo)" -ge 1 ]'
check "the restart request is logged" 'grep -q "stopped code=\"Restart\" exit_code=75" <<< "$(logged)"'

elapsed_ms=$(ct sh -c 's=$(date +%s%N); systemctl stop udp-echo; e=$(date +%s%N); echo $(( (e - s) / 1000000 ))')
echo "systemctl stop took ${elapsed_ms} ms"
check "stop finished in < 4500 ms" '[ "$elapsed_ms" -lt 4500 ]'
check "stopped run exited with status 0" \
  'grep "udp-echo.service: Main process exited" <<< "$(journal)" | tail -n 1 | grep -c "code=exited, status=0/SUCCESS" > /dev/null'
check "SIGTERM and graceful stop logged" \
  'grep -q "phase=\"stopping\" reason=SIGTERM" <<< "$(logged)" && tail -n 1 <<< "$(logged)" | grep -q "stopped code=\"Ok\" exit_code=0"'
check "no SIGKILL" '! grep "udp-echo" <<< "$(journal)" | grep -iE "sigkill|code=killed"'
echo "--- systemd records"
journal | grep -E "udp-echo.service: (Main process exited|Deactivated|Scheduled restart)" || true
echo "--- udp-echo log"
logged
[ "$failures" -eq 0 ] || { echo "hosting/systemd: $failures failure(s)"; exit 1; }
echo "hosting/systemd: passed"
