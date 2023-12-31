# Operation log


## Introduction

Jujutsu records each operation that modifies the repo in the "operation log".
You can see the log with `jj op log`. Each operation object contains a snapshot
of how the repo looked at the end of the operation. We call this snapshot a
"view" object. The view contains information about where each branch, tag, and
Git ref (in Git-backed repos) pointed, as well as the set of heads in the repo,
and the current working-copy commit in each workspace. The operation object also
(in addition to the view) contains pointers to the operation(s) immediately
before it, as well as metadata about the operation, such as timestamps,
username, hostname, description.

The operation log allows you to undo an operation (`jj [op] undo`), which doesn't
need to be the most recent one. It also lets you restore the entire repo to the
way it looked at an earlier point (`jj op restore`).

When referring to operations, you can use `@` to represent the current
operation.

The following operators are supported:

* `x-`: Parents of `x` (e.g. `@-`)
* `x+`: Children of `x`


## Concurrent operations

One benefit of the operation log (and the reason for its creation) is that it
allows lock-free concurrency -- you can run concurrent `jj` commands without
corrupting the repo, even if you run the commands on different machines that
access the repo via a distributed file system (as long as the file system
guarantees that a write is only visible once previous writes are visible). When
you run a `jj` command, it will start by loading the repo at the latest
operation. It will not see any changes written by concurrent commands. If there
are conflicts, you will be informed of them by subsequent `jj st` and/or
`jj log` commands.

As an example, let's say you had started editing the description of a change and
then also update the contents of the change (maybe because you had forgotten the
editor). When you eventually close your editor, the command will succeed and
e.g. `jj log` will indicate that the change has diverged.


## Loading an old version of the repo

The top-level `--at-operation/--at-op` option allows you to load the repo at a
specific operation. This can be useful for understanding how your repo got into
the current state. It can be even more useful for understanding why someone
else's repo got into its current state.

When you use `--at-op`, the automatic snapshotting of the working copy will not
take place. When referring to a revision with the `@` symbol (as many commands
do by default), that will resolve to the working-copy commit recorded in the
operation's view (which is actually how it always works -- it's just the
snapshotting that's skipped with `--at-op`).

As a top-level option, `--at-op` can be passed to any command. However, you
will typically only want to run read-only commands. For example, `jj log`,
`jj st`, and `jj diff` all make sense. It's still possible to run e.g.
`jj --at-op=<some operation ID> describe`. That's equivalent to having started
`jj describe` back when the specified operation was the most recent operation
and then let it run until now (which can be done for that particular command by
not closing the editor). There's practically no good reason to do that other
than to simulate concurrent commands.
