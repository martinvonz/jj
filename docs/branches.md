# Branches


## Introduction

Branches are named pointers to revisions (just like they are in Git). You can
move them without affecting the target revision's identity. Branches
automatically move when revisions are rewritten (e.g. by `jj rebase`). You can
pass a branch's name to commands that want a revision as argument. For example,
`jj co main` will check out the revision pointed to by the "main" branch. Use
`jj branch list` to list branches and `jj branch` to create, move, or delete
branches. There is currently no concept of an active/current/checked-out branch.


## Remotes

Jujutsu identifies a branch by its name across remotes (this is unlike Git and
more like Mercurial's "bookmarks"). For example, a branch called "main" in your
local repo is considered the same branch as a branch by the same name on a
remote. When you pull from a remote (currently only via `jj git fetch`), any
branches from the remote will be imported as branches in your local repo.

Jujutsu also records the last seen position on each remote (just like Git's
remote-tracking branches). You can refer to these with
`<branch name>@<remote name>`, such as `jj co main@origin`. Most commands don't
show the remote branch if it has the same target as the local branch. The local
branch (without `@<remote name>`) is considered the branch's desired target.
Consequently, if you want to update a branch on a remote, you first update the
branch locally and then push the update to the remote.

When you pull from a remote, any changes compared to the current record of the
remote's state will be propagated to the local branch. Let's say you run
`jj git fetch --remote origin` and the remote's "main" branch has moved so its
target is now ahead of the local record in `main@origin`. That will update
`main@origin` to the new target. It will also apply the change to the local
branch `main`. If the local target had also moved compared to `main@origin`
(probably because you had run `jj branch set main`), then the two updates will be
merged. If one is ahead of the other, then that target will be the new target.
Otherwise, the local branch will be conflicted (see next section for details).


## Conflicts

Branches can end up in a conflicted state. When that happens, `jj status` will
include information about the conflicted branches (and instructions for how to
mitigate it). `jj branch list` will have details. `jj log` will show the branch
name with a question mark suffix (e.g. `main?`) on each of the conflicted
branch's potential target revisions. Using the branch name to look up a revision
will resolve to all potential targets. That means that `jj co main` will error
out, complaining that the revset resolved to multiple revisions.

Both local branches (e.g. `main`) and the remote branch (e.g. `main@origin`) can
have conflicts. Both can end up in that state if concurrent operations were run
in the repo. The local branch more typically becomes conflicted because it was
updated both locally and on a remote.

To resolve a conflicted state in a local branch (e.g. `main`), you can move the
branch to the desired target with `jj branch`. You may want to first either
merge the conflicted targets with `jj merge`, or you may want to rebase one side
on top of the other with `jj rebase`.

To resolve a conflicted state in a remote branch (e.g. `main@origin`), simply
pull from the remote (e.g. `jj git fetch`). The conflict resolution will also
propagate to the local branch (which was presumably also conflicted).
