#!/bin/bash
set -euo pipefail
. "$(dirname "$0")"/demo_helpers.sh
parse_args "$@"

new_tmp_dir
jj git clone https://github.com/octocat/Hello-World
cd Hello-World

run_demo 'Basic conflict resolution flow' '
run_command "# We are on the master branch of the octocat/Hello-World repo:"
run_command "jj log"
pause 7
run_command "# Let'\''s make an edit that will conflict when we rebase it:"
run_command "jj describe -m \"README: be specific about which world\""
run_command "echo \"Hello Earth!\" > README"
run_command "jj diff"
pause 2
run_command "# We'\''re going to rebase it onto commit b1. That commit looks like this:"
run_command "jj diff -r b1"
pause 2
run_command "# Now rebase:"
run_command "jj rebase -d b1"
run_command "# Huh, that seemed to succeed. Let'\''s take a look at the repo:"
pause 3
run_command "jj log"
pause 5
run_command "# As you can see, the rebased commit has a conflict. The working copy is on top of the conflict."
run_command "# The file in the working copy looks like this:"
run_command "cat README"
pause 5
run_command "# Now we will resolve the conflict:"
run_command "echo \"Hello earth!\" > README"
pause 2
run_command "# The diff of the conflict resolution looks like this:"
run_command "jj diff"
pause 5
run_command "# We now squash the conflict resolution into the conflicted parent change:"
run_command "jj squash"
pause 2
run_command "# Looks good now:"
run_command "jj log"
pause 3
run_command "jj diff"
'
