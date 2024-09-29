#!/bin/bash
set -euo pipefail
. "$(dirname "$0")"/helpers.sh

new_tmp_dir

comment "Clone a Git repo:"
run_command "jj git clone https://github.com/octocat/Hello-World"
run_command "cd Hello-World"

blank

comment "By default, \"jj\" creates a local bookmark \"master\" tracking the remote master
branch. Other remote branches are only available as remote-tracking bookmarks."
run_command "jj bookmark list --all"
comment "We can create a local bookmark tracking one of the remote branches we just
fetched."
run_command "jj bookmark track octocat-patch-1@origin"

comment "By default, \"jj log\" excludes untracked remote branches to focus on
\"our\" commits."
run_command "jj log"

comment "We can also ask \"jj\" to show all the commits."
run_command "jj log -r 'all()'"

comment "We can look at the diffs of commits in the repo"
run_command "jj diff -r b1"
blank
run_command "jj diff -r b3"

comment "The repo is backed by the actual Git repo:"
run_command "git --git-dir=.jj/repo/store/git log --graph --all --decorate --oneline"

blank
