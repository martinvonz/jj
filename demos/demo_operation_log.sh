#!/bin/bash
set -euo pipefail
. "$(dirname "$0")"/demo_helpers.sh

new_tmp_dir
jj git clone https://github.com/octocat/Hello-World
cd Hello-World

comment "We are in the octocat/Hello-World repo.
The \"operation log\" shows the operations
so far:"
run_command "jj op log"

comment "We are going to make some changes to show
how the operation log works.
We are currently working off of the \"master\"
branch:"
run_command "jj log"

comment "Let's add a file, set a description, and
rebase onto the \"test\" branch:"
run_command "echo stuff > new-file"
run_command "jj describe -m stuff"
run_command "jj rebase -d test"

comment "We are now going to make another change off of
master:"
sleep 1
run_command "jj co master"
run_command "jj describe -m \"other stuff\""

comment "The repo now looks like this:"
run_command "jj log"
run_command "# And the operation log looks like this:"

comment "Let's undo that rebase operation:"
rebase_op=$(jj --color=never op log | grep 'o ' | sed '3q;d' | cut -b3-15)
run_command "jj undo $rebase_op"

comment "The \"stuff\" change is now back on master as
expected:"
run_command "jj log"

comment "We can also see what the repo looked like
after the rebase operation:"
run_command "jj --at-op $rebase_op log"

comment "Looks nice, let's go back to that point:"
run_command "jj op restore $rebase_op"

comment "We're now back to before the \"other stuff\"
change existed:"
run_command "jj log"
