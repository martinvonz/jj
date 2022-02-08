# Jujutsu


## Disclaimer

This is not a Google product. It is an experimental version-control system
(VCS). It was written by me, Martin von Zweigbergk (martinvonz@google.com). It
is my personal hobby project and my 20% project at Google. It does not indicate
any commitment or direction from Google.


## Introduction

Jujutsu is a [Git-compatible](docs/git-compatibility.md)
[DVCS](https://en.wikipedia.org/wiki/Distributed_version_control). It combines
features from Git (data model,
[speed](https://github.com/martinvonz/jj/discussions/49)), Mercurial (anonymous
branching, simple CLI [free from "the index"](docs/git-comparison.md#the-index),
[revsets](docs/revsets.md), powerful history-rewriting), and Pijul/Darcs
([first-class conflicts](docs/conflicts.md)), with features not found in either
of them ([working-copy-as-a-commit](docs/working-copy.md),
[undo functionality](docs/operation-log.md), automatic rebase,
[safe replication via `rsync`, Dropbox, or distributed file
system](docs/technical/concurrency.md)).

The command-line tool is called `jj` for now because it's easy to type and easy
to replace (rare in English). The project is called "Jujutsu" because it matches
"jj".

## Features

### Compatible with Git
   
Jujutsu has two backends. One of them is a Git backend (the other is a native
one). This lets you use Jujutsu as an alternative interface to Git. The commits
you create will look like regular Git commits. You can always switch back to
Git.

<a href="https://asciinema.org/a/DRCzktCyEAxH6j788ZDT6aSjS" target="_blank">
  <img src="https://asciinema.org/a/DRCzktCyEAxH6j788ZDT6aSjS.svg" />
</a>

### The working copy is automatically committed

Most Jujutsu commands automatically commit the working copy. This leads to a
simpler and more powerful interface, since all commands work the same way on the
working copy or any other commit. It also means that you can always check out a
different commit without first explicitly committing the working copy changes
(you can even check out a different commit while resolving merge conflicts).

<a href="https://asciinema.org/a/zWMv4ffmoXykBtrxvDY6ohEaZ" target="_blank">
  <img src="https://asciinema.org/a/zWMv4ffmoXykBtrxvDY6ohEaZ.svg" />
</a>

### Operations update the repo first, then possibly the working copy

The working copy is only updated at the end of an operation, after all other
changes have already been recorded. This means that you can run any command 
(such as `jj rebase`) even if the working copy is dirty.

### Entire repo is under version control

All operations you perform in the repo are recorded, along with a snapshot of
the repo state after the operation. This means that you can easily revert to an
earlier repo state, or to simply undo a particular operation (which does not
necessarily have to be the most recent operation).

<a href="https://asciinema.org/a/OFOTcm2XlZ09LLEI5bHYM8Alw" target="_blank">
  <img src="https://asciinema.org/a/OFOTcm2XlZ09LLEI5bHYM8Alw.svg" />
</a>

### Conflicts can be recorded in commits

If an operation results in conflicts, information about those conflicts will be
recorded in the commit(s). The operation will succeed. You can then resolve the
conflicts later. One consequence of this design is that there's no need to
continue interrupted operations. Instead, you get a single workflow for
resolving conflicts, regardless of which command caused them. This design also
lets Jujutsu rebase merge commits correctly (unlike both Git and Mercurial).

Basic conflict resolution:
<a href="https://asciinema.org/a/MWQz2nAprRXevQEYtaHScN2tJ" target="_blank">
  <img src="https://asciinema.org/a/MWQz2nAprRXevQEYtaHScN2tJ.svg" />
</a>

Juggling conflicts:
<a href="https://asciinema.org/a/HqYA9SL2tzarPAErpYs684GGR" target="_blank">
  <img src="https://asciinema.org/a/HqYA9SL2tzarPAErpYs684GGR.svg" />
</a>

### Automatic rebase

Whenever you modify a commit, any descendants of the old commit will be rebased
onto the new commit. Thanks to the conflict design described above, that can be
done even if there are conflicts. Branches pointing to rebased commits will be
updated. So will the working copy if it points to a rebased commit.

### Comprehensive support for rewriting history

Besides the usual rebase command, there's `jj describe` for editing the
description (commit message) of an arbitrary commit. There's also `jj edit`,
which lets you edit the changes in a commit without checking it out. To split
a commit into two, use `jj split`. You can even move part of the changes in a
commit to any other commit using `jj move`. 


## Status

The tool is quite feature-complete, but some important features like (the
equivalent of) `git blame` and `git log <paths>` are not yet supported. There
are also several performance bugs. It's also likely that workflows and setups
different from what I personally use are not well supported. For example,
pull-request workflows currently require too many manual steps.

I have almost exclusively used `jj` to develop the project itself since early
January 2021. I haven't had to re-clone from source (I don't think I've even had
to restore from backup).

There *will* be changes to workflows and backward-incompatible changes to the
on-disk formats before version 1.0.0. Even the binary's name may change (i.e.
away from `jj`). For any format changes, I'll try to implement transparent
upgrades (as I've done with recent changes), or provide upgrade commands or
scripts if requested.


## Installation

```shell script
# We need the "nightly" Rust toolchain. This command installs that without
# changing your default.
$ rustup install nightly
$ cargo +nightly install --git https://github.com/martinvonz/jj.git
```

To set up command-line completion, source the output of 
`jj debug completion --bash/--zsh/--fish`. For example, if you use Bash:
```shell script
$ source <(jj debug completion)  # --bash is the default
```

You may also want to configure your name and email so commits are made in your
name. Create a `~/.jjconfig` file and make it look something like this:
```shell script
$ cat ~/.jjconfig
[user]
name = "Martin von Zweigbergk"
email = "martinvonz@google.com"
```


## Getting started

The best way to get started is probably to go through
[the tutorial](docs/tutorial.md). Also see the
[Git comparison](docs/git-comparison.md), which includes a table of
`jj` vs. `git` commands.


## Related work

There are several tools trying to solve similar problems as Jujutsu. See
[related work](docs/related-work.md) for details.
