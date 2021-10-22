#!/bin/bash
set -euo pipefail
. "$(dirname "$0")"/demo_helpers.sh
parse_args "$@"

new_tmp_dir
jj git clone https://github.com/octocat/Hello-World
cd Hello-World

run_demo 'The working copy is automatically committed' '
run_command "# We are in the octocat/Hello-World repo."
run_command "# We have an empty working copy on top of master:"
run_command "jj log"
sleep 5
run_command "jj status"
sleep 2
run_command "# Now make some changes in the working copy:"
run_command "echo \"Goodbye World!\" > README"
run_command "echo stuff > new-file"
run_command "# Our working copy commit id changed because we made changes:"
run_command "jj status"
sleep 5
run_command "# Add a branch so we can easily refer to this commit:"
run_command "jj branch goodbye"
sleep 2
run_command "# Start working on a new change off of master:"
run_command "jj co master"
sleep 2
run_command "# Note that the working copy is now clean; the \"goodbye\" change stayed in its own commit:"
run_command "jj status"
sleep 5
run_command "# Modify a file in this new change:"
run_command "echo \"Hello everyone!\" > README"
sleep 2
run_command "# The working copy is not special; we can, for example, set the description of any commit."
run_command "# First, set it on the working copy:"
run_command "jj describe -m everyone"
sleep 2
run_command "# Now set it on the change we worked on before:"
run_command "jj describe goodbye -m goodbye"
sleep 2
run_command "# Inspect the result:"
run_command "jj log"
'
