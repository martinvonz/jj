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

run_command_allow_broken_pipe() {
  run_command "$@" || {
    EXITCODE="$?"
    case $EXITCODE in
    3)
      # `jj` exits with error coded 3 on broken pipe,
      # which can happen simply because of running
      # `jj|head`.
      return 0;;
    *)
      return $EXITCODE;;
    esac
  }
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
