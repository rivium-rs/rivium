#!/usr/bin/env bash
# Hosting test under launchd (run by hosting/launchd on a CI macOS runner; it registers a
# LaunchAgent, so it refuses to run outside CI). An example program runs the way the deployment
# installs a service: started without arguments from its directory, which is its root, next to a
# config.toml that only sets its address, as an agent like the deployment's (RunAtLoad,
# KeepAlive, no ExitTimeOut). Asserts: a restart request exits with 75 and KeepAlive restarts the
# service; SIGTERM from launchd ends a run gracefully with exit code 0; `launchctl bootout` stops
# the service gracefully within 5 s. The programs differ in their configuration and in how they
# are asked to restart (see scripts/hosting/systemd.sh): udp-echo by a datagram, edge-lite by a
# new configuration through its API that injects unit panics, which the restarted run isolates.
# Usage: scripts/hosting/launchd.sh <udp-echo or edge-lite binary> [extra run arguments]
# Checks are passed as strings and evaluated later, hence single quotes.
# shellcheck disable=SC2016,SC2034
set -euo pipefail
[ "${CI:-}" = true ] || { echo "launchd.sh registers a LaunchAgent; it only runs in CI (CI=true)" >&2; exit 2; }
src=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
program=$(basename "$src")
shift
label=io.github.rivium-rs.$program
dir=$(mktemp -d)
bin=$dir/$program
log=$dir/logs/$program/$program.log
port=7357
extra=""
for arg in "$@"; do extra="$extra<string>$arg</string>"; done
plist="$HOME/Library/LaunchAgents/$label.plist"
failures=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; failures=$((failures + 1)); fi; }
now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time * 1000'; }
count() { grep -c -- "$1" "$log" 2> /dev/null || true; }
wait_for() { # <seconds> <condition>
  local deadline=$(($(date +%s) + $1))
  until eval "$2"; do
    [ "$(date +%s)" -lt "$deadline" ] || return 1
    sleep 0.2
  done
}

case "$program" in
  udp-echo)
    config=$(printf '[echo]\naddr = "127.0.0.1:%s"\n' "$port")
    request_restart() { printf restart > "/dev/udp/127.0.0.1/$port"; }
    restarted() { :; }
    ;;
  edge-lite)
    config=$(printf '[http]\naddr = "127.0.0.1:%s"\n' "$port")
    request_restart() { # PUT /api/config: the new file turns on injected unit panics
      local response
      response=$(printf '%s\n\n[faults]\npanic_every = 2\n' "$config" |
        curl -sS -X PUT --data-binary @- "http://127.0.0.1:$port/api/config")
      check "the API accepted the new configuration" 'grep -q "\"status\":\"success\"" <<< "$response"'
    }
    restarted() {
      check "the restarted run isolates unit panics" \
        'wait_for 30 "grep -qE \"a unit failed: it starts again unit=[0-9]+ failures=[0-9]+ why=.panic: a panic injected\" \"\$log\""'
      sleep 2
      check "the panics do not end the run" '[ "$(runs)" -eq 2 ]'
    }
    ;;
  *) echo "launchd.sh: no hosting profile for $program" >&2; exit 2 ;;
esac

cp "$src" "$bin"
printf '%s\n' "$config" > "$dir/config.toml"
mkdir -p "$HOME/Library/LaunchAgents"
cat > "$plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$label</string>
  <key>ProgramArguments</key>
  <array><string>$bin</string>$extra</array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>$dir/stdout.log</string>
  <key>StandardErrorPath</key><string>$dir/stderr.log</string>
</dict>
</plist>
EOF
domain="gui/$(id -u)"
launchctl bootstrap "$domain" "$plist" 2> /dev/null || { domain="user/$(id -u)"; launchctl bootstrap "$domain" "$plist"; }
cleanup() { launchctl bootout "$domain/$label" 2> /dev/null || true; rm -f "$plist"; }
trap cleanup EXIT
echo "bootstrapped $label in $domain"
info() { launchctl print "$domain/$label" 2> /dev/null || true; }
pid_of() { info | awk '$1 == "pid" && $2 == "=" { print $3 }'; }
runs() { count 'phase changed phase="running"'; }
gone() { ! kill -0 "$1" 2> /dev/null; }

check "first run is running" 'wait_for 30 "[ \$(runs) -ge 1 ]"'
request_restart
# The run exits with 75; KeepAlive restarts it after the throttle interval (10 s).
check "exit 75 is restarted by KeepAlive" 'wait_for 40 "[ \$(runs) -ge 2 ]"'
check "the restart request is logged" 'grep -q "stopped code=\"Restart\" exit_code=75" "$log"'
restarted
info | grep -E "last exit code|runs =" || true

pid=$(pid_of)
start=$(now_ms)
launchctl kill SIGTERM "$domain/$label"
check "SIGTERM run exits" 'wait_for 10 "gone $pid"'
echo "exit after SIGTERM took $(($(now_ms) - start)) ms"
check "graceful stop logged" 'grep -q "phase=\"stopping\" reason=SIGTERM" "$log" && [ "$(count "stopped code=\"Ok\" exit_code=0")" -eq 1 ]'
check "last exit code is 0" 'grep -qE "last exit code = 0$" <<< "$(info)"'

check "restarted again after the clean exit" 'wait_for 40 "[ \$(runs) -ge 3 ]"'
pid=$(pid_of)
start=$(now_ms)
launchctl bootout "$domain/$label"
check "bootout: process gone" 'wait_for 10 "gone $pid"'
elapsed=$(($(now_ms) - start))
echo "bootout took $elapsed ms"
check "bootout within 5000 ms" '[ "$elapsed" -lt 5000 ]'
check "bootout run stopped gracefully" '[ "$(count "stopped code=\"Ok\" exit_code=0")" -eq 2 ]'
echo "--- $program log"
cat "$log" || true
echo "--- stderr"
cat "$dir/stderr.log" || true
[ "$failures" -eq 0 ] || { echo "hosting/launchd: $failures failure(s)"; exit 1; }
echo "hosting/launchd: passed"
