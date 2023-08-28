#!/bin/bash
set -euo pipefail
. "$(dirname "$0")"/helpers.sh

new_tmp_dir
jj git clone https://github.com/octocat/Hello-World
cd Hello-World

comment "We are in the octocat/Hello-World repo.
We have an empty working copy on top of master:"
run_command "jj status"
run_command "jj log"

comment "Now make some changes in the working copy:"
run_command "echo \"Goodbye World!\" > README"
run_command "echo stuff > new-file"

comment "Our working copy's commit ID changed
because we made changes:"
run_command "jj status"
run_command "jj log"

comment "Add a branch so we can easily refer to this
commit:"
run_command "jj branch create goodbye"
run_command "jj log"

comment "Start working on a new change off of master:"
run_command "jj co master"
run_command "jj log"

comment "Note that the working copy is now clean; the
\"goodbye\" change stayed in its own commit:"
run_command "jj status"

comment "Modify a file in this new change:"
run_command "echo \"Hello everyone!\" > README"

comment "The working copy is not special; we can, for
example, set the description of any commit.
First, set it on the working copy:"
run_command "jj describe -m everyone"

comment "Now set it on the change we worked on before:"
run_command "jj describe goodbye -m goodbye"

comment "Inspect the result:"
run_command "jj log"

blank
