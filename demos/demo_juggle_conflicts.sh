#!/bin/bash
set -euo pipefail
. "$(dirname "$0")"/helpers.sh

new_tmp_dir
(
    jj init --config-toml ui.allow-init-native=true
    echo "first" > file
    jj bookmark create first
    jj commit -m 'first'
    echo "second" > file
    jj bookmark create second
    jj commit -m 'second'
    echo "third" > file
    jj bookmark create third
    jj commit -m 'third'
) >/dev/null 2>&1

comment "We are in a repo with three commits, all
editing the same line:"
run_command "jj log"

run_command "jj diff -r first"
run_command "jj diff -r second"
run_command "jj diff -r third"

comment "Let's reorder the second and third commits:"
run_command_output_redacted "jj rebase -s third -d first"
run_command "jj rebase -s second -d third"
run_command "jj log"
comment "The commit labeled \"third\" has a conflict, as expected. What's more
interesting is that the top commit has no conflict! That's because it
has the changes from all three commits applied to it.

Let's verify that by looking at its contents:"
run_command "jj new second"
run_command "cat file"

comment "Let's now instead make \"second\" and \"third\"
sibling and merge them:"
run_command "jj rebase -s second -d first"
run_command "jj new second third -m merged"
run_command "jj log"
comment "Again, because the merge commit has the
changes from all three commits, it has no
conflict."

blank
