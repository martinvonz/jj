# Bookmarks


## Introduction

Bookmarks are named pointers to revisions (just like branches are in Git). You 
can move them without affecting the target revision's identity. Bookmarks
automatically move when revisions are rewritten (e.g. by `jj rebase`). You can
pass a bookmark's name to commands that want a revision as argument. For example,
`jj new main` will create a new revision on top of the "main" bookmark. Use
`jj bookmark list` to list bookmarks and `jj bookmark` to create, move, or delete
bookmarks. There is currently no concept of an active/current/checked-out bookmark.

Currently Jujutsu maps its Bookmarks to Git Branches and stores them as that 
in the Git backend. This means that all Bookmarks will be reflected as 
Git Branches, this may change in the future. 

## Remotes and tracked bookmarks

Jujutsu records the last seen position of a bookmark on each remote (just like
Git's remote-tracking branches). This record is updated on every `jj git fetch`
and `jj git push` of the bookmark. You can refer to the remembered remote bookmark
positions with `<bookmark name>@<remote name>`, such as `jj new main@origin`. `jj`
does not provide a way to manually edit these recorded positions.

A remote bookmark can be associated with a local bookmark of the same name. This is
called a **tracked remote bookmark**, which currently maps to a Git remote 
branch. When you pull a tracked bookmark from a remote, any changes compared to
the current record of the remote's state will be propagated to the corresponding
local bookmark, which will be created if it doesn't exist already.

!!! note "Details: how `fetch` pulls bookmarks"

    Let's say you run `jj git fetch --remote origin` and, during the fetch, `jj`
    determines that the remote's "main" bookmark has been moved so that its target is
    now ahead of the local record in `main@origin`.

    `jj` will then update `main@origin` to the new target. If `main@origin` is
    **tracked**, `jj` will also apply the change to the local bookmark `main`. If the
    local target has also been moved compared to `main@origin` (probably because you
    ran `jj bookmark set main`), then the two updates will be merged. If one is ahead
    of the other, then that target will become the new target. Otherwise, the local
    bookmark will become conflicted (see the ["Conflicts" section](#conflicts) below
    for details).

Most commands don't show the tracked remote bookmark if it has the same target as
the local bookmark. The local bookmark (without `@<remote name>`) is considered the
bookmark's desired target. Consequently, if you want to update a bookmark on a
remote, you first update the bookmark locally and then push the update to the
remote. If a local bookmark also exists on some remote but points to a different
target there, `jj log` will show the bookmark name with an asterisk suffix (e.g.
`main*`). That is meant to remind you that you may want to push the bookmark to
some remote.

If you want to know the internals of bookmark tracking, consult the 
[Design Doc][design].

### Terminology summary

- A **remote bookmark** is a bookmark ref on the remote. `jj` can find out its
  actual state only when it's actively communicating with the remote. However,
  `jj` does store the last-seen position of the remote bookmark; this is the
  commit `jj show <bookmark name>@<remote name>` would show. This notion is
  completely analogous to Git's "remote-tracking bookmarks".
- A **tracked (remote) bookmark** is defined above. You can make a remote bookmark
  tracked with the [`jj bookmark track` command](#manually-tracking-a-bookmark), for
  example.
- A **tracking (local) bookmark** is the local bookmark that `jj` tries to keep in
  sync with the tracked remote bookmark. For example, after `jj bookmark track
  mybookmark@origin`, there will be a local bookmark `mybookmark` that's tracking the
  remote `mybookmark@origin` bookmark. A local bookmark can track a bookmark of the same
  name on 0 or more remotes.

The notion of tracked bookmarks serves a similar function to the Git notion of an
"upstream branch". Unlike Git, a single local bookmark can be tracking remote
bookmarks on multiple remotes, and the names of the local and remote bookmarks
must match.

### Manually tracking a bookmark

To track a bookmark permanently use `jj bookmark track <bookmark name>@<remote name>`. 
It will now be imported as a local bookmark until you untrack it or it is deleted
on the remote. 

Example:

```sh
$ # List all available bookmarks, as we want our colleague's bookmark.
$ jj bookmark list --all
$ # Find the bookmark.
$ # [...]
$ # Actually track the bookmark.
$ jj bookmark track <bookmark name>@<remote name> # Example: jj bookmark track my-feature@origin
$ # From this point on, <bookmark name> will be imported when fetching from <remote name>.
$ jj git fetch --remote <remote name>
$ # A local bookmark <bookmark name> should have been created or updated while fetching.
$ jj new <bookmark name> # Do some local testing, etc.
```

### Untracking a bookmark

To stop following a remote bookmark, you can `jj bookmark untrack` it. After that,
subsequent fetches of that remote will no longer move the local bookmark to match
the position of the remote bookmark.

Example: 

```sh
$ # List all local and remote bookmarks.
$ jj bookmark list --all
$ # Find the bookmark we no longer want to track.
$ # [...]
# # Actually untrack it.
$ jj bookmark untrack <bookmark name>@<remote name> # Example: jj bookmark untrack stuff@origin
$ # From this point on, this remote bookmark won't be imported anymore.
$ # The local bookmark (e.g. stuff) is unaffected. It may or may not still
$ # be tracking bookmarks on other remotes (e.g. stuff@upstream).
```

### Listing tracked bookmarks

To list tracked bookmarks, you can `jj bookmark list --tracked` or `jj bookmark list -t`.
This command omits local Git-tracking bookmarks by default.

You can see if a specific bookmark is tracked with `jj bookmark list --tracked <bookmark name>`.


### Automatic tracking of bookmarks & `git.auto-local-bookmark` option

There are two situations where `jj` tracks bookmarks automatically. `jj git
clone` automatically sets up the default remote bookmark (e.g. `main@origin`) as
tracked. When you push a local bookmark, the newly created bookmark on the remote is
marked as tracked.

By default, every other remote bookmark is marked as "not tracked" when it's
fetched. If desired, you need to manually `jj bookmark track` them. This works
well for repositories where multiple people work on a large number of bookmarks. 

The default can be changed by setting the config `git.auto-local-bookmark = true`.
Then, `jj git fetch` tracks every *newly fetched* bookmark with a local bookmark.
Branches that already existed before the `jj git fetch` are not affected. This
is similar to Mercurial, which fetches all its bookmarks (equivalent to Git
bookmarks) by default.

## Bookmark movement

Currently Jujutsu automatically moves local bookmarks when these conditions are
met:

 * When a commit has been rewritten (e.g, when you rebase) bookmarks and the  
   working-copy will move along with it.
 * When a commit has been abandoned, all associated bookmarks will be moved 
   to its parent(s). If a working copy was pointing to the abandoned commit,
   then a new working-copy commit will be created on top of the parent(s).

You could describe the movement as following along the change-id of the 
current bookmark commit, even if it isn't entirely accurate.

## Pushing bookmarks: Safety checks

Before `jj git push` actually moves, creates, or deletes a remote bookmark, it
makes several safety checks.

1. `jj` will contact the remote and check that the actual state of the remote
   bookmark matches `jj`'s record of its last known position. If there is a
   conflict, `jj` will refuse to push the bookmark. In this case, you need to run
   `jj git fetch --remote <remote name>` and resolve the resulting bookmark
   conflict. Then, you can try `jj git push` again.

   If you are familiar with Git, this makes `jj git push` similar to `git
   push --force-with-lease`.

   There are a few cases where `jj git push` will succeed even though the remote
   bookmark is in an unexpected location. These are the cases where `jj git fetch`
   would not create a bookmark conflict and would not move the local bookmark, e.g.
   if the unexpected location is identical to the local position of the bookmark.

2. The local bookmark must not be [conflicted](#conflicts). If it is, you would
   need to use `jj bookmark set`, for example, to resolve the conflict.

   This makes `jj git push` safe even if `jj git fetch` is performed on a timer
   in the background (this situation is a known issue[^known-issue] with some
   forms of `git push --force-with-lease`). If the bookmark moves on a remote in a
   problematic way, `jj git fetch` will create a conflict. This should ensure
   that the user becomes aware of the conflict before they can `jj git push` and
   override the bookmark on the remote.

3. If the remote bookmark already exists on the remote, it must be
   [tracked](#remotes-and-tracked-bookmarks). If the bookmark does not already
   exist on the remote, there is no problem; `jj git push` will create the
   remote bookmark and mark it as tracked.

[^known-issue]: See "A general note on safety" in
    <https://git-scm.com/docs/git-push#Documentation/git-push.txt---no-force-with-lease>


## Conflicts

Bookmarks can end up in a conflicted state. When that happens, `jj status` will
include information about the conflicted bookmarks (and instructions for how to
mitigate it). `jj bookmark list` will have details. `jj log` will show the bookmark
name with a double question mark suffix (e.g. `main??`) on each of the
conflicted bookmark's potential target revisions. Using the bookmark name to look up
a revision will resolve to all potential targets. That means that `jj new main`
will error out, complaining that the revset resolved to multiple revisions.

Both local bookmarks (e.g. `main`) and the remote bookmark (e.g. `main@origin`) can
have conflicts. Both can end up in that state if concurrent operations were run
in the repo. The local bookmark more typically becomes conflicted because it was
updated both locally and on a remote.

To resolve a conflicted state in a local bookmark (e.g. `main`), you can move the
bookmark to the desired target with `jj bookmark move`. You may want to first either
merge the conflicted targets with `jj new` (e.g. `jj new 'all:main'`), or you may
want to rebase one side on top of the other with `jj rebase`.

To resolve a conflicted state in a remote bookmark (e.g. `main@origin`), simply
pull from the remote (e.g. `jj git fetch`). The conflict resolution will also
propagate to the local bookmark (which was presumably also conflicted).

## Ease of use

The use of bookmarks is frequent in some workflows, for example, when
interacting with Git repositories containing branches. To this end,
one-letter shortcuts have been implemented, both for the `jj bookmark`
command itself through an alias (as `jj b`), and for its subcommands.
For example, `jj bookmark create BOOKMARK-NAME` can be abbreviated as
`jj b c BOOKMARK-NAME`.

[design]: design/tracking-branches.md
