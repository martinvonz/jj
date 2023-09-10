#!/bin/bash
set -euo pipefail
. "$(dirname "$0")"/helpers.sh

new_tmp_dir
{
    jj git clone https://github.com/octocat/Hello-World 
    cd Hello-World
    jj abandon test
    jj branch forget test
} > /dev/null

comment "We are on the master branch of the
octocat/Hello-World repo:"
run_command "jj log"

comment "Let's make an edit that will conflict
when we rebase it:"
run_command "jj describe -m \"README: say which world\""
run_command "echo \"Hello Earth!\" > README"
run_command "jj diff"

# TODO(ilyagr): Get the real shortest prefix of the b1b commit using `jj log
# --no-graph` and the `.shortest()` template function.
#
# This could also be done in demo_git_compat.sh, but that might not be worth it.
comment "We're going to rebase it onto commit b1.
That commit looks like this:"
run_command "jj diff -r b1"

comment "Now rebase:"
run_command "jj rebase -d b1"

comment "That seemed to succeed but we are also told there is now a conflict.
Let's take a look at the repo:"
run_command "jj log"
run_command "jj status"

comment "Indeed, the rebased commit has a conflict. The conflicted file
in the working copy looks like this:"
run_command "cat README"

comment "Now we will resolve the conflict:"
run_command "echo \"Hello earth!\" > README"

comment "The status command no longer reports it:"
run_command "jj status"

blank
