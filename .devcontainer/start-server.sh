#!/usr/bin/env bash
set -euo pipefail

# The lock prevents duplicate servers when this hook is run again.
# Restrict access because the server log contains the pairing code.
umask 077
log_dir=/tmp/nio-de-codespaces
mkdir -p "$log_dir"
nohup flock --nonblock "$log_dir/server.lock" \
  nio-de --no-cloudflare >> "$log_dir/server.log" 2>&1 < /dev/null &
printf 'NioDE startup requested. Read the latest pairing code with:\n  tail -n 80 %s/server.log\n' "$log_dir"
