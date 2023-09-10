#!/bin/bash
set -euo pipefail
. "$(dirname "$0")"/helpers.sh

new_tmp_dir

comment "Clone a Git repo:"
run_command "jj git clone https://github.com/octocat/Hello-World"
run_command "cd Hello-World"

comment "Inspect it:"
run_command "jj log -r 'all()'"
blank
run_command "jj diff -r b1"

comment "The repo is backed by the actual Git repo:"
run_command "git --git-dir=.jj/repo/store/git log --graph --all --decorate --oneline"

blank
