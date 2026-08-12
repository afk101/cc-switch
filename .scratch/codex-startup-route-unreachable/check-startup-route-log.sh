#!/bin/sh

set -eu

log_file=${1:-/Users/qihoo/.cc-switch/logs/cc-switch.log}
profile_name=${2:-codex-api}

failure_line=$(rg -n "operation=startup_restore .*profile_name=${profile_name} " "$log_file" | tail -n 1 || true)

if [ -z "$failure_line" ]; then
  printf 'GREEN: %s 没有启动恢复失败记录\n' "$profile_name"
  exit 0
fi

failure_number=${failure_line%%:*}
listener_line=$(tail -n "+$((failure_number + 1))" "$log_file" | rg -n "Profile .* 已监听 127\\.0\\.0\\.1:" | head -n 1 || true)

printf 'RED: %s 启动恢复失败，失败记录：%s\n' "$profile_name" "$failure_line"
if [ -n "$listener_line" ]; then
  printf '之后首次监听记录（需要后续生命周期动作）：%s\n' "$listener_line"
else
  printf '之后没有监听成功记录\n'
fi
exit 1
