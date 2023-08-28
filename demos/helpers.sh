#!/bin/bash
set -euo pipefail

new_tmp_dir() {
    local dirname
    dirname=$(mktemp -d)
    mkdir -p "$dirname"
    cd "$dirname"
    trap "rm -rf '$dirname'" EXIT
}

run_command() {
  echo "\$ $@"
  eval "$@"
}

blank() {
  echo ""
}

comment() {
  indented="$(echo "$@"| sed 's/^/# /g')"
  blank
  echo -e "\033[0;32m${indented}\033[0m"
  blank
}
