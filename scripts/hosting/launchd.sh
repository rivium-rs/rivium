#!/usr/bin/env bash
# Hosting smoke under launchd (run by hosting/launchd on a CI macOS runner; it registers a
# LaunchAgent, so it refuses to run outside CI). The agent matches the deployment's: RunAtLoad,
# KeepAlive, no ExitTimeOut. Asserts: a run that exits with 75 is restarted by KeepAlive; SIGTERM
# from launchd ends a run gracefully with exit code 0; `launchctl bootout` stops the service
# within 5 s.
# Usage: scripts/hosting/launchd.sh <smoke binary> [extra serve arguments]
# Checks are passed as strings and evaluated later, hence single quotes.
# shellcheck disable=SC2016,SC2034
set -euo pipefail
[ "${CI:-}" = true ] || { echo "launchd.sh registers a LaunchAgent; it only runs in CI (CI=true)" >&2; exit 2; }
bin=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
shift
extra=""
for arg in "$@"; do extra="$extra<string>$arg</string>"; done
label=io.github.rivium-rs.smoke
dir=$(mktemp -d)
plist="$HOME/Library/LaunchAgents/$label.plist"
failures=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; failures=$((failures + 1)); fi; }
now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time * 1000'; }
count() { grep -c -- "$1" "$2" 2> /dev/null || true; }
wait_for() { # <seconds> <condition>
  local deadline=$(($(date +%s) + $1))
  until eval "$2"; do
    [ "$(date +%s)" -lt "$deadline" ] || return 1
    sleep 0.2
  done
}

mkdir -p "$HOME/Library/LaunchAgents"
cat > "$plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$label</string>
  <key>ProgramArguments</key>
  <array>
    <string>$bin</string><string>serve</string>
    <string>--log</string><string>$dir/smoke.log</string>
    <string>--exit-once</string><string>75</string>
    <string>--marker</string><string>$dir/exited</string>$extra
  </array>
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

ready_runs() { count " ready$" "$dir/smoke.log"; }
gone() { ! kill -0 "$1" 2> /dev/null; }

# The first run exits with 75; KeepAlive restarts it after the throttle interval (10 s).
check "exit 75 is restarted by KeepAlive" 'wait_for 40 "[ \$(ready_runs) -ge 1 ]"'
check "two runs started" '[ "$(count started "$dir/smoke.log")" -eq 2 ]'
info | grep -E "last exit code|runs =" || true

pid=$(pid_of)
start=$(now_ms)
launchctl kill SIGTERM "$domain/$label"
check "SIGTERM run exits" 'wait_for 10 "gone $pid"'
echo "exit after SIGTERM took $(($(now_ms) - start)) ms"
check "graceful stop logged" 'grep -q "stop signal=SIGTERM" "$dir/smoke.log" && grep -q " stopped$" "$dir/smoke.log"'
check "last exit code is 0" 'grep -qE "last exit code = 0$" <<< "$(info)"'

check "restarted again after the clean exit" 'wait_for 40 "[ \$(ready_runs) -ge 2 ]"'
pid=$(pid_of)
start=$(now_ms)
launchctl bootout "$domain/$label"
check "bootout: process gone" 'wait_for 10 "gone $pid"'
elapsed=$(($(now_ms) - start))
echo "bootout took $elapsed ms"
check "bootout within 5000 ms" '[ "$elapsed" -lt 5000 ]'
check "bootout run stopped gracefully" '[ "$(count " stopped$" "$dir/smoke.log")" -eq 2 ]'
echo "--- smoke log"
cat "$dir/smoke.log" || true
[ "$failures" -eq 0 ] || { echo "hosting/launchd: $failures failure(s)"; exit 1; }
echo "hosting/launchd: passed"
