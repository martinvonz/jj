#!/bin/bash
set -euo pipefail
. "$(dirname "$0")"/helpers.sh

new_tmp_dir
{
    jj git clone https://github.com/octocat/Hello-World
    cd Hello-World
    jj abandon test
    jj branch forget test
    jj abandon octocat-patch-1
    jj branch forget octocat-patch-1
}> /dev/null

comment "We are in the octocat/Hello-World repo.
We have an empty working copy on top of master:"
run_command "jj log"
run_command "jj status"

comment "Now make some changes in the working copy:"
run_command "echo \"Goodbye World!\" > README"
run_command "echo stuff > new-file"

comment "Because of these changes, our working copy is no longer marked as \"(empty)\".
Also, its commit ID (starting with a blue character) changed:"
run_command "jj status"

comment "Add a branch so we can easily refer to this
commit:"
run_command "jj branch create goodbye"
run_command "jj log"

comment "Start working on a new change off of master:"
run_command "jj co master"
comment "Note that we were told the working copy is now empty (AKA clean). The
\"goodbye\" change stayed in its own commit:"

run_command "jj log"
comment "Let's do a sanity check: 'jj status' should tell us that
the working copy is clean."
run_command "jj status"

comment "Modify a file in this new change:"
run_command "echo \"Hello everyone!\" > README"
run_command "jj status"

comment "The working copy is not special; we can, for
example, set the description of any commit.
First, set it on the working copy:"
# The output with the description of the working copy slightly messes up the
# parallel between the working copy and another commit, so we redact it.
run_command_output_redacted "jj describe -m everyone"

comment "Now set it on the change we worked on before:"
run_command "jj describe goodbye -m goodbye"

comment "Inspect the result:"
run_command "jj log"
