# Jujube


## Disclaimer

This is not a Google product. It is an experimental version-control system
(VCS). It is not ready for use. It was written by me, Martin von Zweigbergk
(martinvonz@google.com). It is my personal hobby project. It does not indicate
any commitment or direction from Google.


## Introduction

I started the project mostly in order to test the viability of some UX ideas in
practice. I continue to use it for that, but my short-term goal now is to make
it useful as an alternative CLI for Git repos.

The storage design is similar to Git's in that it stores commits, trees, and
blobs. However, the blobs are actually split into three types: normal files,
symlinks (Unicode paths), and conflicts (more about that later).

The command-line tool is called `jj` for now because it's easy to type and easy
to replace (rare in English). The project is called "Jujube" (a fruit) because
that's the first word I could think of that matched "jj".


## Features

The following subsections describe the current features. The text is aimed at
readers who are already familiar with other VCSs.

### Compatible with Git

The tool currently has two backends. One is called "local store" and is very
simple and inefficient. The other backend uses a Git repo as storage. The
commits are stored as regular Git commits. Commits can be read from and written
to an existing Git repo. This makes it possible to create a Jujube repo and use
it as an alternative interface for a Git repo (it will be backed by the Git repo
just like additional Git worktrees are).

### Written as a library

The project consists of two main parts: the lib crate and the main (CLI)
crate. Most of the code lives in the lib crate. The lib crate does not print
anything to the terminal. The separate lib crate should make it relatively
straight-forward to add a GUI.

### Operations are performed repo-first

Almost all operations are done in the repo first and then possibly reflected in
the working copy. The only exception so far is when committing the working copy,
which naturally uses the working copy as input.

This makes it faster because the working copy doesn't need to get updated. It
also means that the working copy won't see spurious changes e.g. during a rebase
operation. It makes it safe to update the working copy while some operation is
running.

### Supports Evolution

Jujube copies the Evolution feature from Mercurial. It keeps track of when a
commit gets rewritten. A commit has a list of predecessors in addition to the
usual list of parents. This lets the tool figure out where to rebase descendant
commits to when a commit has been rewritten (amended, rebased, etc.). See
https://www.mercurial-scm.org/wiki/ChangesetEvolution for more information.

### The working copy is a commit

The working copy gets automatically committed when you interact with the
tool. This simplifies both implementation and UX. It also means that the working
copy is frequently backed up.

Any changes to the working copy stay in place when you check out another
commit. That is different from Git and Mercurial, but I think it's more
intuitive for new users. To replicate the default behavior of Git/Mercurial, use
`jj rebase -r @ -d <destination>` (`@` is a name for the working copy
commit). There is no need to stash/unstash.

Commands become more consistent because the same command can operate on the repo
or another commit. For example, `jj log` includes the working copy (much like
`gitk` and other tools include a node for the working copy). `jj squash`
squashes a commit into its parent, including if it's the working copy (like `git
commit --amend`/`hg amend`).

A commit description can be added to the working copy before "commit". The same
command (`jj describe`) is used for changing the description of any commit.

### Commits can contain conflicts

When a merge conflict happens, it is recorded within the tree object as a
special conflict object (not a file object with conflict markers). Conflicts are
stored as a lists of states to add and another list of states to remove. A
regular 3-way merge adds [B,C] and removes [A] (the common ancestor). A
modify/remove conflict adds [B] and removes [A]. An add/add conflict adds
[B,C]. An octopus merge of N commits adds N states and removes N-1 states. A
non-conflict state A is equivalent to a conflict state that just adds [A]. A
"state" here can be a normal file, a symlink, or a tree. This support for
in-tree conflicts has some interesting effects on both implementation and UX.

It means that there is a consistent way of resolving conflicts: check out a
commit with conflicts in, resolve the conflicts, and amend them into the
conflicted commit. Then evolve descendant commits.

It naturally enables collaborative conflict resolution.

The in-tree conflicts means that there is no need for book-keeping in
rebase-like commands to support continue/abort operations. Instead, the rebase
can simply continue and create the desired new DAG shape.

Conflicts get simplified on rebase by removing pairs of matching states in the
"add" and "remove" lists. For example, let's say commit B is based on A and is
rebased to C, where it results in conflicts, which the user leaves
unresolved. If the commit is then rebased to D, it will be a regular 3-way merge
between B and D with A as base (no trace of C). This means that you can keep old
commits rebased to head without resolving conflicts, and you still won't have
messy recursive conflicts.

The conflict handling also results in some Darcs-/Pijul-like properties. For
example, if you rebase a commit and it results in conflicts, and you then back
out that commit, the conflict will go away. (I plan to make that work even if
there had been unrelated changes in the file, but I haven't gotten around to it
yet.)

The criss-cross merge case becomes simpler. In Git, the virtual ancestor may
have conflicts and you may get nested conflict markers in the working copy. In
Jujube, the result is a merge with multiple parts, which may even get simplified
to not be recursive.

The in-tree conflicts make it natural and easy to define the contents of a merge
commit to be the difference compared to the merged parents (the so-called "evil"
part of the merge), so that's what Jujube does. Rebasing merge commits therefore
works as you would expect (Git and Mercurial both handle rebasing of merge
commits poorly). It's even possible to change the number of parents while
rebasing, so if A is non-merge commit, you can make it a merge commit with `jj
rebase -r A -d B -d C`. `jj diff -r <commit>` will show you the diff compared to
the merged parents.

I intend for commands that present the contents of a tree (such as listing
files) to use the "add" state(s) of the conflict, but that's not yet done.

### Operations are logged

Each write operation is logged to a content-addressed storage, much like the
commit storage. The Operation object has an associated View object, much like
the Commit object has a Tree object. The view object contains all the heads
currently in the repo, as well as the checked-out commit. It will also contain
the refs if I add support for that. The operation object can have multiple
parent operations, so it forms a DAG just like the commit graph does. There is
normally only one parent operation, but there can be multiple parents if
concurrent operations happened.

I added the operation log as a solution for the problem of making concurrent
repo edits safe. When the repo is loaded, it is loaded at a particular
operation, which provides an immutable view of the repo. For a caller of the
library to start making changes, they then have to start a transaction. Once
they are done making changes to the transaction, they commit the
transaction. The operation object is then created. This step cannot fail (except
if the file system runs out of space or such). Pointers to the heads of the
operation DAG are kept as files in a directory (the filename is the operation
id). When a new operation object has been created, its operation id is added to
the directory. The transaction's base operation id is then removed from that
directory. If concurrent operations happened, there would be multiple new
operation ids in the directory and only one base operation id would have been
removed. If a reader sees the repo in this state, it will attempt to merge the
views and create a new operation with multiple parents. If there are conflicts,
the user will have to resolve it (I haven't implemented that yet).

As a nice side-effect of adding the operation log to solve the concurrent-edits
problem, we get some very useful UX features. Many UX features come from mapping
commands that work on the commit graph onto the operation graph. For example, if
you map `git revert`/`hg backout` onto the operation graph, you get an operation
that undoes a previous operation (called `jj op undo`). Note that any operation
can be undone, not just the latest one. If you map `git restore`/`hg revert`
onto the operation graph, you get an operation that rewinds the repo state to an
earlier point (called `jj op restore`).

You can also see what the repo looked like at an earlier point with `jj
--at-op=<operation id> log`. As mentioned earlier, the checkout is also part of
the view, so that command will show you where the working copy was at that
operation. If you do `jj op restore -o <operation id>`, it will also update the
working copy accordingly. This is actually how the working copy is always
updated: we first commit a transaction with a pointer to the new checkout and
then the working copy is updated to reflect that.

## Future plans

TODO
