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
  # `bash` often resets $COLUMNS, so we also
  # allow $RUN_COMMAND_COLUMNS
  COLUMNS=${RUN_COMMAND_COLUMNS-${COLUMNS-80}} eval "$@"
}

run_command_output_redacted() {
  echo "\$ $@"
  # `bash` often resets $COLUMNS, so we also
  # allow $RUN_COMMAND_COLUMNS
  eval "$@" > /dev/null
  echo -e "\033[0;90m... (output redacted) ...\033[0m"
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
