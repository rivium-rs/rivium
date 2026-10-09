#!/usr/bin/env bash
# snmp-lite answers net-snmp's command-line tools, which encode and decode BER on their own: the
# slice's tests use its SNMP code on both ends, this checks it against another implementation
# (ci/slices). Runs the program below a new root and asks it with snmpget, snmpgetnext and
# snmpwalk; another community gets no answer; SIGTERM stops it with 0.
# Usage: scripts/snmp-interop.sh <snmp-lite program>    Needs net-snmp's tools (package snmp).
set -euo pipefail
bin=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
for tool in snmpget snmpgetnext snmpwalk; do
  command -v "$tool" > /dev/null || { echo "snmp-interop: $tool is required (net-snmp)" >&2; exit 2; }
done
root=$(mktemp -d)
"$bin" --root "$root" --set snmp.addr=127.0.0.1:0 --set device.serial=SN-INTEROP > "$root/out" 2>&1 &
pid=$!
trap 'kill "$pid" 2> /dev/null || true; rm -rf "$root"' EXIT
addr=""
for _ in $(seq 100); do
  addr=$(sed -n -E 's/.* listening .*listen\.addr=([^ ]+).*/\1/p' "$root/logs/snmp-lite/snmp-lite.log" 2> /dev/null || true)
  [ -z "$addr" ] || break
  sleep 0.1
done
[ -n "$addr" ] || { echo "snmp-interop: the agent did not start" >&2; cat "$root/out" >&2; exit 1; }
# Numeric names only: net-snmp reads no MIB files.
export MIBS=""
failures=0
ask() { # <tool> <community> <name>...: the tool's output, and its status as the last line
  local tool=$1 community=$2 status=0 output
  shift 2
  output=$("$tool" -v2c -c "$community" -On -t 2 -r 0 "udp:$addr" "$@" 2>&1) || status=$?
  # net-snmp tells of the directories it creates on its first run: that is not an answer.
  grep -v '^Created directory: ' <<< "$output" || true
  echo "status $status"
}
check() { # <what> <output> <extended regular expression for each line>...
  local what=$1 output=$2
  shift 2
  if [ "$(wc -l <<< "$output")" -eq $# ] && paste -d '\n' <(printf '%s\n' "$@") <(printf '%s\n' "$output") |
    awk 'NR % 2 == 1 { re = $0; next } $0 !~ ("^" re "$") { bad = 1 } END { exit bad }'; then
    echo "ok   $what"
  else
    echo "FAIL $what:"
    printf '%s\n' "$output"
    failures=$((failures + 1))
  fi
}

version=$("$bin" --version)
check "snmpget: the system group and the serial number" \
  "$(ask snmpget public .1.3.6.1.2.1.1.1.0 .1.3.6.1.2.1.1.2.0 .1.3.6.1.4.1.32473.1.1.0)" \
  "\.1\.3\.6\.1\.2\.1\.1\.1\.0 = STRING: \"$version\"" \
  '\.1\.3\.6\.1\.2\.1\.1\.2\.0 = OID: \.1\.3\.6\.1\.4\.1\.32473\.1' \
  '\.1\.3\.6\.1\.4\.1\.32473\.1\.1\.0 = STRING: "SN-INTEROP"' 'status 0'
check "snmpgetnext: the reading follows the serial number" \
  "$(ask snmpgetnext public .1.3.6.1.4.1.32473.1.1.0)" \
  '\.1\.3\.6\.1\.4\.1\.32473\.1\.2\.0 = INTEGER: [0-9]+' 'status 0'
check "snmpget: an object the agent does not have" \
  "$(ask snmpget public .1.3.6.1.2.1.2.1.0)" \
  '\.1\.3\.6\.1\.2\.1\.2\.1\.0 = No Such Object available on this agent at this OID' 'status 0'
check "snmpwalk: every object, in order, to the end of the view" \
  "$(ask snmpwalk public .1.3.6.1)" \
  "\.1\.3\.6\.1\.2\.1\.1\.1\.0 = STRING: \"$version\"" \
  '\.1\.3\.6\.1\.2\.1\.1\.2\.0 = OID: \.1\.3\.6\.1\.4\.1\.32473\.1' \
  '\.1\.3\.6\.1\.2\.1\.1\.3\.0 = Timeticks: \([0-9]+\) .*' \
  '\.1\.3\.6\.1\.4\.1\.32473\.1\.1\.0 = STRING: "SN-INTEROP"' \
  '\.1\.3\.6\.1\.4\.1\.32473\.1\.2\.0 = INTEGER: [0-9]+' \
  '\.1\.3\.6\.1\.4\.1\.32473\.1\.3\.0 = Counter32: [0-9]+' \
  '\.1\.3\.6\.1\.4\.1\.32473\.1\.4\.0 = Counter32: [0-9]+' \
  '\.1\.3\.6\.1\.4\.1\.32473\.1\.4\.0 = No more variables left in this MIB View \(It is past the end of the MIB tree\)' \
  'status 0'
check "another community gets no answer" \
  "$(ask snmpget private .1.3.6.1.4.1.32473.1.1.0)" \
  "Timeout: No Response from udp:$addr\.?" 'status 1'

kill -TERM "$pid"
status=0
wait "$pid" || status=$?
if [ "$status" -eq 0 ]; then echo "ok   SIGTERM stops the agent with 0"; else
  echo "FAIL SIGTERM: exit $status"
  failures=$((failures + 1))
fi
[ "$failures" -eq 0 ] || { echo "snmp-interop: $failures failure(s)"; exit 1; }
echo "snmp-interop: passed"
