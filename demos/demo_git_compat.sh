#!/bin/bash
set -euo pipefail
. "$(dirname "$0")"/demo_helpers.sh
parse_args "$@"

new_tmp_dir

run_demo '
expect_prompt
run_command "# Clone a Git repo:"
expect_prompt
run_command "jj git clone https://github.com/octocat/Hello-World"
expect_prompt
run_command "cd Hello-World"
expect_prompt
run_command "# Inspect it:"
expect_prompt
run_command "jj log"
expect_prompt
run_command "jj diff -r b1"
expect_prompt
run_command "# The repo is backed by the actual Git repo:"
expect_prompt
run_command "git --git-dir=.jj/store/git log --graph --all --decorate --oneline"
'
