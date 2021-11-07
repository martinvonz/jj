#!/bin/bash
set -euo pipefail
. "$(dirname "$0")"/demo_helpers.sh
parse_args "$@"

new_tmp_dir
jj init
echo "first" > file
jj branch first
jj close -m 'first' 
echo "second" > file
jj branch second
jj close -m 'second' 
echo "third" > file
jj branch third
jj close -m 'third' 

run_demo 'Juggling conflicts' '
run_command "# We are in a repo with three commits, all"
run_command "# editing the same line:"
run_command "jj log"
pause 3

run_command "jj diff -r first"
pause 1
run_command "jj diff -r second"
pause 1
run_command "jj diff -r third"
run_command ""
pause 2

run_command "# Let'\''s reorder the second and third commits:"
run_command "jj rebase -s third -d first"
run_command "jj rebase -s second -d third"
run_command "jj log"
pause 3
run_command "# The commit labeled \"third\" has a conflict,"
run_command "# as expected. What'\''s more interesting is"
run_command "# that the top commit has no conflict! That'\''s"
run_command "# because it has the changes from all three"
run_command "# commits applied to it."
run_command ""
pause 5

run_command "# Let'\''s verify that by looking at its contents:"
run_command "jj co second"
run_command "cat file"
run_command ""
pause 3

run_command "# Let'\''s now instead make \"second\" and \"third\""
run_command "# sibling and merge them:"
run_command "jj rebase -s second -d first"
run_command "jj merge second third -m merged"
run_command "jj log"
pause 3
run_command "# Again, because the merge commit has the"
run_command "# changes from all three commits, it has no"
run_command "# conflict."
'
