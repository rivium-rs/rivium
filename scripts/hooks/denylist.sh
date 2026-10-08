# shellcheck shell=bash
# Sourced by the hooks: resolves the private denylist or refuses to continue (fail closed).
# Set it with `just hooks <path>` (git config rivium.denylist) or the RIVIUM_DENYLIST variable.
denylist=${RIVIUM_DENYLIST:-$(git config --get rivium.denylist || true)}
if [ -z "$denylist" ] || [ ! -s "$denylist" ]; then
  echo "public-content check: no denylist configured (run \`just hooks <path-to-private-denylist>\`)" >&2
  exit 1
fi
