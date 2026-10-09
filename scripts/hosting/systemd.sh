#!/usr/bin/env bash
# Hosting test under systemd (run by hosting/systemd; also runs locally with Docker). An example
# program runs as a service the way the deployment installs one: started without arguments from
# its directory, which is its root, next to a config.toml that only sets its address, under a
# unit like the deployment's (Type=simple, Restart=always, no RestartSec, TimeoutStopSec or
# KillSignal) inside a systemd container. Asserts: a restart request exits with 75 and systemd
# restarts the service; `systemctl stop` stops it gracefully (SIGTERM logged, exit status 0,
# under 4.5 s, no SIGKILL). The programs differ in their configuration and in how they are asked
# to restart:
#   udp-echo   the datagram `restart`;
#   edge-lite  a new configuration through its API, which injects unit panics: the restarted run
#              must keep running through them (a release build isolates panics).
# Usage: scripts/hosting/systemd.sh <udp-echo or edge-lite binary for the Docker host; its glibc
#        must not exceed the base image's (RIVIUM_SYSTEMD_BASE, default ubuntu:24.04)> [extra run arguments]
# Checks are passed as strings and evaluated later, hence single quotes.
# shellcheck disable=SC2016,SC2034
set -euo pipefail
bin=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
program=$(basename "$bin")
shift
extra=${*:-}
base=${RIVIUM_SYSTEMD_BASE:-ubuntu:24.04}
image=rivium-systemd-hosting
name=rivium-systemd-hosting-$$
dir=/opt/$program
log=$dir/logs/$program/$program.log
port=7357
failures=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; failures=$((failures + 1)); fi; }

case "$program" in
  udp-echo)
    config=$(printf '[echo]\naddr = "127.0.0.1:%s"\n' "$port")
    request_restart() { ct bash -c "printf restart > /dev/udp/127.0.0.1/$port"; }
    restarted() { :; }
    ;;
  edge-lite)
    config=$(printf '[http]\naddr = "127.0.0.1:%s"\n' "$port")
    request_restart() { # PUT /api/config: the new file turns on injected unit panics
      local body response
      body=$(printf '%s\n\n[faults]\npanic_every = 2\n' "$config")
      response=$(printf 'PUT /api/config HTTP/1.1\r\nHost: test\r\nContent-Length: %s\r\nConnection: close\r\n\r\n%s' \
        "${#body}" "$body" | ct bash -c "exec 3<>/dev/tcp/127.0.0.1/$port; cat >&3; cat <&3")
      check "the API accepted the new configuration" 'grep -q "\"status\":\"success\"" <<< "$response"'
    }
    restarted() {
      check "the restarted run isolates unit panics" \
        'wait_logged 30 "a unit failed: it starts again unit=[0-9]+ failures=[0-9]+ why=\"panic: a panic injected"'
      sleep 2
      check "the panics do not end the run" '[ "$(runs)" -eq 2 ] && [ "$(ct systemctl is-active "$program")" = active ]'
    }
    ;;
  *) echo "systemd.sh: no hosting profile for $program" >&2; exit 2 ;;
esac

docker build -q -t "$image" - > /dev/null <<EOF
FROM $base
RUN apt-get update && apt-get install -y --no-install-recommends systemd && rm -rf /var/lib/apt/lists/*
CMD ["/lib/systemd/systemd"]
EOF
docker run -d --name "$name" --privileged --cgroupns=private --tmpfs /run --tmpfs /run/lock \
  -v "$bin":"$dir/$program":ro "$image" > /dev/null
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

printf '%s\n' "$config" | ct sh -c "cat > $dir/config.toml"
ct sh -c "cat > /etc/systemd/system/$program.service" <<EOF
[Unit]
Description=Rivium hosting test

[Service]
Type=simple
ExecStart=$dir/$program $extra
Restart=always
EOF
ct systemctl daemon-reload
ct systemctl start "$program"
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
wait_logged() { # <seconds> <extended regular expression>: waits for a line of the log
  for _ in $(seq $(($1 * 5))); do
    grep -qE "$2" <<< "$(logged)" && return 0
    sleep 0.2
  done
  return 1
}
journal() { ct journalctl -o cat; }

check "first run is running" 'wait_runs 1'
request_restart
# The run exits with 75; systemd restarts it (RestartSec defaults to 100 ms).
check "restarted run is running" 'wait_runs 2'
check "the restart request exited with 75" 'grep -q "$program.service: Main process exited, code=exited, status=75/" <<< "$(journal)"'
check "exit 75 is restarted (NRestarts >= 1)" '[ "$(ct systemctl show -p NRestarts --value "$program")" -ge 1 ]'
check "the restart request is logged" 'grep -q "stopped code=\"Restart\" exit_code=75" <<< "$(logged)"'
restarted

elapsed_ms=$(ct sh -c "s=\$(date +%s%N); systemctl stop $program; e=\$(date +%s%N); echo \$(( (e - s) / 1000000 ))")
echo "systemctl stop took ${elapsed_ms} ms"
check "stop finished in < 4500 ms" '[ "$elapsed_ms" -lt 4500 ]'
check "stopped run exited with status 0" \
  'grep "$program.service: Main process exited" <<< "$(journal)" | tail -n 1 | grep -c "code=exited, status=0/SUCCESS" > /dev/null'
check "SIGTERM and graceful stop logged" \
  'grep -q "phase=\"stopping\" reason=SIGTERM" <<< "$(logged)" && tail -n 1 <<< "$(logged)" | grep -q "stopped code=\"Ok\" exit_code=0"'
check "no SIGKILL" '! grep "$program" <<< "$(journal)" | grep -iE "sigkill|code=killed"'
echo "--- systemd records"
journal | grep -E "$program.service: (Main process exited|Deactivated|Scheduled restart)" || true
echo "--- $program log"
logged
[ "$failures" -eq 0 ] || { echo "hosting/systemd: $failures failure(s)"; exit 1; }
echo "hosting/systemd: passed"
