#!/bin/bash
set -euo pipefail
. "$(dirname "$0")"/demo_helpers.sh
parse_args "$@"

new_tmp_dir

run_demo '
run_command "# Clone a Git repo:"
run_command "jj git clone https://github.com/octocat/Hello-World"
run_command "cd Hello-World"
run_command "# Inspect it:"
run_command "jj log"
run_command "jj diff -r b1"
run_command "# The repo is backed by the actual Git repo:"
run_command "git --git-dir=.jj/store/git log --graph --all --decorate --oneline"
'
