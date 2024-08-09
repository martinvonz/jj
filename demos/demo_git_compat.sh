#!/bin/bash
set -euo pipefail
. "$(dirname "$0")"/helpers.sh

new_tmp_dir

comment "Clone a Git repo:"
run_command "jj git clone https://github.com/octocat/Hello-World"
run_command "cd Hello-World"

comment "By default, \"jj\" creates a local master branch tracking the remote master
branch. The other branches are only available as remote-tracking branches."
run_command "jj branch list --all"
comment "We can create a local branch tracking one of the remote branches we just
fetched."
run_command "jj branch track octocat-patch-1@origin"

comment "By default, \"jj log\" shows the commits jj considers \"ours\" together
with their parents."
run_command "jj log"

comment "We can also ask \"jj\" to show all the commits."
run_command "jj log -r 'all()'"

comment "We can look at the commits in the repo"
run_command "jj diff -r b1"
run_command "jj diff -r b3"

comment "The repo is backed by the actual Git repo:"
run_command "git --git-dir=.jj/repo/store/git log --graph --all --decorate --oneline"

blank
