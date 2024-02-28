# Branches


## Introduction

Branches are named pointers to revisions (just like they are in Git). You can
move them without affecting the target revision's identity. Branches
automatically move when revisions are rewritten (e.g. by `jj rebase`). You can
pass a branch's name to commands that want a revision as argument. For example,
`jj co main` will check out the revision pointed to by the "main" branch. Use
`jj branch list` to list branches and `jj branch` to create, move, or delete
branches. There is currently no concept of an active/current/checked-out branch.

## Remotes and tracked branches

Jujutsu records the last seen position of a branch on each remote (just like
Git's remote-tracking branches). This record is updated on every `jj git fetch`
and `jj git push` of the branch. You can refer to the remembered remote branch
positions with `<branch name>@<remote name>`, such as `jj new main@origin`. `jj`
does not provide a way to manually edit these recorded positions.

A remote branch can be associated with a local branch of the same name. This is
called a **tracked remote branch**. When you pull a tracked branch from a
remote, any changes compared to the current record of the remote's state will be
propagated to the corresponding local branch, which will be created if it
doesn't exist already.

!!! note "Details: how `fetch` pulls branches"

    Let's say you run `jj git fetch --remote origin` and, during the fetch, `jj`
    determines that the remote's "main" branch has been moved so that its target is
    now ahead of the local record in `main@origin`.

    `jj` will then update `main@origin` to the new target. If `main@origin` is
    **tracked**, `jj` will also apply the change to the local branch `main`. If the
    local target has also been moved compared to `main@origin` (probably because you
    ran `jj branch set main`), then the two updates will be merged. If one is ahead
    of the other, then that target will become the new target. Otherwise, the local
    branch will become conflicted (see the ["Conflicts" section](#conflicts) below
    for details).

Most commands don't show the tracked remote branch if it has the same target as
the local branch. The local branch (without `@<remote name>`) is considered the
branch's desired target. Consequently, if you want to update a branch on a
remote, you first update the branch locally and then push the update to the
remote. If a local branch also exists on some remote but points to a different
target there, `jj log` will show the branch name with an asterisk suffix (e.g.
`main*`). That is meant to remind you that you may want to push the branch to
some remote.

If you want to know the internals of branch tracking, consult the 
[Design Doc][design].

### Terminology summary

- A **remote branch** is a branch ref on the remote. `jj` can find out its
  actual state only when it's actively communicating with the remote. However,
  `jj` does store the last-seen position of the remote branch; this is the
  commit `jj show <branch name>@<remote name>` would show. This notion is
  completely analogous to Git's "remote-tracking branches".
- A **tracked (remote) branch** is defined above. You can make a remote branch
  tracked with the [`jj branch track` command](#manually-tracking-a-branch), for
  example.
- A **tracking (local) branch** is the local branch that `jj` tries to keep in
  sync with the tracked remote branch. For example, after `jj branch track
  mybranch@origin`, there will be a local branch `mybranch` that's tracking the
  remote `mybranch@origin` branch. A local branch can track a branch of the same
  name on 0 or more remotes.

The notion of tracked branches serves a similar function to the Git notion of an
"upstream branch". Unlike Git, a single local branch can be tracking remote
branches on multiple remotes, and the names of the local and remote branches
must match.

### Manually tracking a branch

To track a branch permanently use `jj branch track <branch name>@<remote name>`. 
It will now be imported as a local branch until you untrack it or it is deleted
on the remote. 

Example:

```sh
$ # List all available branches, as we want our colleague's branch.
$ jj branch list --all
$ # Find the branch.
$ # [...]
$ # Actually track the branch.
$ jj branch track <branch name>@<remote name> # Example: jj branch track my-feature@origin
$ # From this point on, <branch name> will be imported when fetching from <remote name>.
$ jj git fetch --remote <remote name>
$ # A local branch <branch name> should have been created or updated while fetching.
$ jj new <branch name> # Do some local testing, etc.
```

### Untracking a branch

To stop following a remote branch, you can `jj branch untrack` it. After that,
subsequent fetches of that remote will no longer move the local branch to match
the position of the remote branch.

Example: 

```sh
$ # List all local and remote branches.
$ jj branch list --all
$ # Find the branch we no longer want to track.
$ # [...]
# # Actually untrack it.
$ jj branch untrack <branch name>@<remote name> # Example: jj branch untrack stuff@origin
$ # From this point on, this remote branch won't be imported anymore.
$ # The local branch (e.g. stuff) is unaffected. It may or may not still
$ # be tracking branches on other remotes (e.g. stuff@upstream).
```

### Listing tracked branches

To list tracked branches, you can `jj branch list --tracked` or `jj branch list -t`.
This command omits local Git-tracking branches by default.

You can see if a specific branch is tracked with `jj branch list --tracked <branch name>`.


### Automatic tracking of branches & `git.auto-local-branch` option

There are two situations where `jj` tracks branches automatically. `jj git
clone` automatically sets up the default remote branch (e.g. `main@origin`) as
tracked. When you push a local branch, the newly created branch on the remote is
marked as tracked.

By default, every other remote branch is marked as "not tracked" when it's
fetched. If desired, you need to manually `jj branch track` them. This works
well for repositories where multiple people work on a large number of branches. 

The default can be changed by setting the config `git.auto-local-branch = true`.
Then, `jj git fetch` tracks every *newly fetched* branch with a local branch.
Branches that already existed before the `jj git fetch` are not affected. This
is similar to Mercurial, which fetches all its bookmarks (equivalent to Git
branches) by default.

## Branch movement

Currently Jujutsu automatically moves local branches when these conditions are
met:

 * When a commit has been rewritten (e.g, when you rebase) branches and the  
   working-copy will move along with it.
 * When a commit has been abandoned, all associated branches will be moved 
   to its parent(s). If a working copy was pointing to the abandoned commit,
   then a new working-copy commit will be created on top of the parent(s).

You could describe the movement as following along the change-id of the 
current branch commit, even if it isn't entirely accurate.

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

[design]: design/tracking-branches.md
