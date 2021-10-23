#!/bin/bash
set -euo pipefail
. "$(dirname "$0")"/demo_helpers.sh
parse_args "$@"

new_tmp_dir
jj git clone https://github.com/octocat/Hello-World
cd Hello-World

run_demo 'The entire repo is under version control' '
run_command "# We are in the octocat/Hello-World repo."
run_command "# The \"operation log\" shows the operations so far:"
run_command "jj op log"
sleep 7
run_command "# We are going to make some changes so we can see how the operation log works."
run_command "# We are currently working off of the \"master\" branch:"
run_command "jj log"
sleep 5
run_command "# Let'\''s add a file, set a description, and rebase onto the \"test\" branch:"
run_command "echo stuff > new-file"
sleep 2
run_command "jj describe -m stuff"
sleep 2
run_command "jj rebase -d test"
sleep 2
run_command "# We are now going to make another change off of master:"
run_command "jj co master"
sleep 1
run_command "jj describe -m \"other stuff\""
sleep 2
run_command "# The repo now looks like this:"
run_command "jj log"
sleep 5
run_command "# And the operation log looks like this:"
send -h "jj op log\r"
# Capture the third latest operation id (skipping color codes around it)
expect -re "o ..34m(.*?)..0m "
expect -re "o ..34m(.*?)..0m "
set rebase_op $expect_out(1,string)
expect_prompt
sleep 7
run_command "# Let'\''s undo that rebase operation:"
run_command "jj undo -o $rebase_op"
sleep 3
run_command "# The \"stuff\" change is now back on master as expected:"
run_command "jj log"
sleep 5
run_command "# We can also see what the repo looked like after the rebase operation:"
run_command "jj --at-op $rebase_op log"
sleep 5
run_command "# Looks nice, let'\''s go back to that point:"
run_command "jj op restore -o $rebase_op"
sleep 2
run_command "# We'\''re now back to before the \"other stuff\" change existed:"
run_command "jj log"
'
