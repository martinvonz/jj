# Concurrency

## Introduction

Concurrent editing is a key feature of DVCSs -- that's why they're called
*Distributed* Version Control Systems. A DVCS that didn't let users edit files
and create commits on separate machines at the same time wouldn't be much
of a distributed VCS.

When conflicting changes are made in different clones, a DVCS will have to deal
with that when you push or pull. For example, when using Mercurial, if the
remote has updated a bookmark called `main` (Mercurial's bookmarks are similar
to a Git's branches) and you had updated the same bookmark locally but made it
point to a different target, Mercurial would add a bookmark called `main@origin`
to indicate the conflict. Git instead prevents the conflict by renaming pulled
branches to `origin/main` whether or not there was a conflict. However, most
DVCSs treat local concurrency quite differently, typically by using lock files
to prevent concurrent edits. Unlike those DVCSs, Jujutsu treats concurrent edits
the same whether they're made locally or remotely.

One problem with using lock files is that they don't work when the clone is in a
distributed file system. Most clones are of course not stored in distributed
file systems, but it is a *big* problem when they are (Mercurial repos
frequently get corrupted, for example).

Another problem with using lock files is related to complexity of
implementation. The simplest way of using lock files is to take coarse-grained
locks early: every command that may modify the repo takes a lock at the very
beginning. However, that means that operations that wouldn't actually conflict
would still have to wait for each other. The user experience can be improved by
using finer-grained locks and/or taking the locks later. The drawback of that is
complexity. For example, you need to verify that any assumptions you made before
locking are still valid after you take the lock.

To avoid depending on lock files, Jujutsu takes a different approach by
accepting that concurrent changes can always happen. It instead exposes any
conflicting changes to the user, much like other DVCSs do for conflicting
changes made remotely.

### Syncing with `rsync`, NFS, Dropbox, etc

Jujutsu's lock-free concurrency means that it's possible to update copies of the
clone on different machines and then let `rsync` (or Dropbox, or NFS, etc.)
merge them. The working copy may mismatch what's supposed to be checked out, but
no changes to the repo will be lost (added commits, moved branches, etc.). If
conflicting changes were made, they will appear as conflicts. For example, if a
branch was moved to two different locations, they will appear in `jj log` in
both locations but with a "?" after the name, and `jj status` will also inform
the user about the conflict.

Note that, for now, there are known bugs in this area. Most notably, with the
Git backend, [repository corruption is possible because the backend is not
entirely lock-free](https://github.com/martinvonz/jj/issues/2193). If you know
about the bug, it is relatively easy to recover from.

Moreover, such use of Jujutsu is not currently thoroughly tested,
especially in the context of [co-located
repositories](../glossary.md#co-located-repos). While the contents of commits
should be safe, concurrent modification of a repository from different computers
might conceivably lose some branch pointers. Note that, unlike in pure
Git, losing a branch pointer does not lead to losing commits.


## Operation log

The most important piece in the lock-free design is the "operation log". That is
what allows us to detect and merge concurrent operations.

The operation log is similar to a commit DAG (such as in
[Git's object model](https://git-scm.com/book/en/v2/Git-Internals-Git-Objects)),
but each commit object is instead an "operation" and each tree object is instead
a "view". The view object contains the set of visible head commits, branches,
tags, and the working-copy commit in each workspace. The operation object
contains a pointer to the view object (like how commit objects point to tree
objects), pointers to parent operation(s) (like how commit objects point to
parent commit(s)), and metadata about the operation. These types are defined
in `op_store.proto` The operation log is normally linear.
It becomes non-linear if there are concurrent operations.

When a command starts, it loads the repo at the latest operation. Because the
associated view object completely defines the repo state, the running command
will not see any changes made by other processes thereafter. When the operation
completes, it is written with the start operation as parent. The operation
cannot fail to commit (except for disk failures and such). It is left for the
next command to notice if there were concurrent operations. It will have to be
able to do that anyway since the concurrent operation could have arrived via a
distributed file system. This model -- where each operation sees a consistent
view of the repo and is guaranteed to be able to commit their changes -- greatly
simplifies the implementation of commands.

It is possible to load the repo at a particular operation with
`jj --at-operation=<operation ID> <command>`. If the command is mutational, that
will result in a fork in the operation log. That works exactly the same as if
any later operations had not existed when the command started. In other words,
running commands on a repo loaded at an earlier operation works the same way as
if the operations had been concurrent. This can be useful for simulating
concurrent operations.

### Merging concurrent operations

If Jujutsu tries to load the repo and finds multiple heads in the operation log,
it will do a 3-way merge of the view objects based on their common ancestor
(possibly several 3-way merges if there were more than two heads). Conflicts
are recorded in the resulting view object. For example, if branch `main` was
moved from commit A to commit B in one operation and moved to commit C in a
concurrent operation, then `main` will be recorded as "moved from A to B or C".
See the `RefTarget` definition in `op_store.proto`.

Because we allow branches (etc.) to be in a conflicted state rather than just
erroring out when there are multiple heads, the user can continue to use the
repo, including performing further operations on the repo. Of course, some
commands will fail when using a conflicted branch. For example,
`jj checkout main` when `main` is in a conflicted state will result in an error
telling you that `main` resolved to multiple revisions.

### Storage

The operation objects and view objects are stored in content-addressed storage
just like Git commits are. That makes them safe to write without locking.

We also need a way of finding the current head of the operation log. We do that
by keeping the ID of the current head(s) as a file in a directory. The ID is the
name of the file; it has no contents. When an operation completes, we add a file
pointing to the new operation and then remove the file pointing to the old
operation. Writing the new file is what makes the operation visible (if the old
file didn't get properly deleted, then future readers will take care of that).
This scheme ensures that transactions are atomic.
