#!/usr/bin/env bash
# Runs a program built on Rivium once and stops it as a supervisor would: below a new root, it
# waits until the services run, sends SIGTERM, and checks that the program logs a clean stop and
# exits with 0. The glibc jobs run it where an artifact will run, such as CentOS 7 (glibc 2.17),
# so that the program runs there, not only its symbol versions are read.
# Usage: scripts/check-stop.sh <program> [run arguments]
set -euo pipefail
bin=$1
shift
root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT
name=$("$bin" --version | cut -d ' ' -f 1)
log=$root/logs/$name/$name.log
"$bin" --root "$root" "$@" > "$root/stdout" 2> "$root/stderr" &
pid=$!
for _ in $(seq 300); do
  if grep -q 'phase changed phase="running"' "$log" 2> /dev/null || ! kill -0 "$pid" 2> /dev/null; then break; fi
  sleep 0.1
done
kill -TERM "$pid" 2> /dev/null || true
status=0
wait "$pid" || status=$?
if [ "$status" -ne 0 ] || ! grep -q 'stopped code="Ok" exit_code=0' "$log"; then
  echo "check-stop: $name exited with $status" >&2
  cat "$log" "$root/stderr" >&2 2> /dev/null || true
  exit 1
fi
echo "check-stop: $name ran and stopped on SIGTERM with 0"
