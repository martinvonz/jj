# Glossary

## Anonymous branch

An anonymous branch is a chain of commits that doesn't have any
[named branches](#branch) pointing to it or to any of its descendants. Unlike
Git, Jujutsu keeps commits on anonymous branches around until they are
explicitly abandoned. Visible anonymous branches are tracked by the
[view](#view), which stores a list of [heads](#head) of such branches.

## Backend

A backend is an implementation of the storage layer. There are currently two
builtin commit backends: the Git backend and the native backend. The Git backend
stores commits in a Git repository. The native backend is used for testing
purposes only. Alternative backends could be used, for example, if somebody
wanted to use jj with a humongous monorepo (as Google does).

There are also pluggable backends for storing other information than commits,
such as the "operation store backend" for storing
[the operation log](#operation-log).

## Branch

A branch is a named pointer to a [commit](#commit). They automatically follow
the commit if it gets [rewritten](#rewrite). Branches are sometimes called
"named branches" to distinguish them from
[anonymous branches](#anonymous-branch), but note that they are more similar
to Git's branches than to
[Mercurial's named branches](https://www.mercurial-scm.org/wiki/Branch#Named_branches).
See [here](branches.md) for details.

## Change

A change is a commit as it [evolves over time](#rewrite).

## Change ID

A change ID is a unique identifier for a [change](#change). They are typically
16 bytes long and are often randomly generated. By default, `jj log` presents
them as a sequence of 12 letters in the k-z range, at the beginning of a line.
These are actually hexadecimal numbers that use "digits" z-k instead of 0-9a-f.

For the git backend, Change IDs are currently maintained only locally and not
exchanged via push/fetch operations.

## Commit

A snapshot of the files in the repository at a given point in time (technically
a [tree object](#tree)), together with some metadata. The metadata includes the
author, the date, and pointers to the commit's parents. Through the pointers to
the parents, the commits form a
[Directed Acyclic Graph (DAG)](https://en.wikipedia.org/wiki/Directed_acyclic_graph)
.

Note that even though commits are stored as snapshots, they are often treated
as differences between snapshots, namely compared to their parent's snapshot. If
they have more than one parent, then the difference is computed against the
result of merging the parents. For example, `jj diff` will show the differences
introduced by a commit compared to its parent(s), and `jj rebase` will apply
those changes onto another base commit.

The word "revision" is used as a synonym for "commit".

## Commit ID

A commit ID is a unique identifier for a [commit](#commit). They are 20 bytes
long when using the Git backend. They are presented in regular hexadecimal
format at the end of the line in `jj log`, using 12 hexadecimal digits by
default. When using the Git backend, the commit ID is the Git commit ID.

## Co-located repos

When using the Git [backend](#backend) and the backing Git repository's `.git/`
directory is a sibling of `.jj/`, we call the repository "co-located". Most
tools designed for Git can be easily used on such repositories. `jj` and `git`
commands can be used interchangeably.

See [here](git-compatibility.md#co-located-jujutsugit-repos) for details.

## Conflict

Conflicts can occur in many places. The most common type is conflicts in files.
Those are the conflicts that users coming from other VCSs are usually familiar
with. You can see them in `jj status` and in `jj log` (the red "conflict"
label at the end of the line). See [here](conflicts.md) for details.

Conflicts can also occur in [branches](#branch). For example, if you moved a
branch locally, and it was also moved on the remote, then the branch will be
in a conflicted state after you pull from the remote.
See [here](branches.md#conflicts) for details.

Similar to a branch conflict, when a [change](#change) is rewritten locally
and remotely, for example, then the change will be in a conflicted state. We
call that a [divergent change](#divergent-change).

## Divergent change

A divergent change is a [change](#change) that has more than one
[visible commit](#visible-commits).

## Head

A head is a commit with no descendants. The context in which it has no
descendants varies. For example, the `heads(X)`
[revset function](revsets.md#functions) returns commits that have no descendants
within the set `X` itself. The [view](#view) records which
anonymous heads (heads without a branch pointing to them) are visible at a
given [operation](#operation). Note that this is quite different from Git's
[HEAD](https://git-scm.com/book/en/v2/Git-Internals-Git-References#ref_the_ref).

## Hidden commits, abandoned commits

See [visible commits](#visible-commits).

## Operation

A snapshot of the [visible commits](#visible-commits) and [branches](#branches)
at a given point in time (technically a [view object](#view)), together with
some metadata. The metadata includes the username, hostname, timestamps, and
pointers to the operation's parents.

## Operation log

The operation log is the
[DAG](https://en.wikipedia.org/wiki/Directed_acyclic_graph) formed by
[operation](#operation) objects, much in the same way that commits form a DAG,
which is sometimes called the "commit history". When operations happen in
sequence, they form a single line in the graph. Operations that happen
concurrently from jj's perspective result in forks and merges in the DAG.

## Repository

Basically everything under `.jj/`, i.e. the full set of [operations](#operation)
and [commits](#commit).

## Remote

TODO

## Revision

A synonym for [Commit](#commit).

## Revset

Jujutsu supports a functional language for selecting a set of revisions.
Expressions in this language are called "revsets". See [here](revsets.md) for
details. We also often use the term "revset" for the set of revisions selected
by a revset.

## Rewrite

To "rewrite" a commit means to create a new version of that commit with
different contents, metadata (including parent pointers), or both. Rewriting a
commit results in a new commit, and thus a new [commit ID](#commit-id), but the
[change ID](#change-id) generally remains the same. Some examples of rewriting a
commit would be changing its description or rebasing it. Modifying the working
copy rewrites the working copy commit.

## Root commit

The root commit is a virtual commit at the root of every repository. It has a
commit ID consisting of all '0's (`00000000...`) and a change ID consisting of
all 'z's (`zzzzzzzz...`). It can be referred to in [revsets](#revset) by the
function `root()`. Note that our definition of "root commit" is different from
Git's; Git's "root commits" are the first commit(s) in the repository, i.e. the
commits `jj log -r root()+` will show.

## Tree

A tree object represents a snapshot of a directory in the repository. Tree
objects are defined recursively; each tree object only has the files and
directories contained directly in the directory it represents.

## Tracked branches and tracking branches

A remote branch can be made "tracked" with the `jj branch track` command. This
results in a "tracking" local branch that tracks the remote branch.

See [the branches documentation](branches.md#terminology-summary) for a more
detailed definition of these terms.

## Visible commits

Visible commits are the commits you see in `jj log -r 'all()'`. They are the
commits that are reachable from an anonymous head in the [view](#view).
Ancestors of a visible commit are implicitly visible.

Intuitively, visible commits are the "latest versions" of a revision with a
given [change id](#change-id). A commit that's abandoned or
[rewritten](#rewrite) stops being visible and is labeled as "hidden". Such
commits are no longer accessible using a change id, but they are still
accessible by their [commit id](#commit-id).

## View

A view is a snapshot of branches and their targets, anonymous heads,
and working-copy commits. The anonymous heads define which commits
are [visible](#visible-commits).

A view object is similar to a [tree](#tree) object in that it represents a
snapshot without history, and an [operation](#operation) object is similar to a
[commit](#commit) object in that it adds metadata and history.

## Workspace

A workspace is a [working copy](#working-copy) and an
associated [repository](#repository). There can be multiple workspaces for a
single repository. Each workspace has a `.jj/` directory, but the
[commits](#commit) and [operations](#operation) will be stored in the initial
workspace; the other workspaces will have pointers to the initial workspace. See
[here](working-copy.md#workspaces) for details.

This is what Git calls a "worktree".

## Working copy

The working copy contains the files you're currently working on. It is
automatically snapshot at the beginning of almost every `jj` command, thus
creating a new [working-copy commit](#working-copy-commit) if any changes had
been made in the working copy. Conversely, the working copy is automatically
updated to the state of the working-copy commit at the end of almost every `jj`
command. See [here](working-copy.md) for details.

This is what Git calls a "working tree".

## Working-copy commit

A commit that corresponds to the current state of the working copy. There is
one working-copy commit per [workspace](#workspace). The current working-copy
commits are tracked in the [operation log](#operation-log).
