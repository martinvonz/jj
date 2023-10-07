<div class="title-block" style="text-align: center;" align="center">

# Jujutsu—a version control system

![](https://img.shields.io/github/license/martinvonz/jj)
![](https://img.shields.io/github/v/release/martinvonz/jj)
![](https://img.shields.io/github/release-date/martinvonz/jj)
![](https://img.shields.io/crates/v/jj-cli)
<br/>
![](https://github.com/martinvonz/jj/workflows/build/badge.svg)
![](https://img.shields.io/codefactor/grade/github/martinvonz/jj/main)
![](https://img.shields.io/librariesio/github/martinvonz/jj)

**[Homepage] &nbsp;&nbsp;&bull;&nbsp;&nbsp;**
**[Installation] &nbsp;&nbsp;&bull;&nbsp;&nbsp;**
**[Getting Started Tutorial]**

[Homepage]: https://martinvonz.github.io/jj
[Installation]: https://martinvonz.github.io/jj/latest/install-and-setup
[Getting Started Tutorial]: https://martinvonz.github.io/jj/latest/tutorial

</div>

## Introduction

Jujutsu is a
[Git-compatible](https://martinvonz.github.io/jj/latest/git-compatibility)
[DVCS](https://en.wikipedia.org/wiki/Distributed_version_control). It combines
features from Git (data model,
[speed](https://github.com/martinvonz/jj/discussions/49)), Mercurial (anonymous
branching, simple CLI [free from "the
index"](https://martinvonz.github.io/jj/latest/git-comparison#the-index),
[revsets](https://martinvonz.github.io/jj/latest/revsets), powerful
history-rewriting), and Pijul/Darcs ([first-class
conflicts](https://martinvonz.github.io/jj/latest/conflicts)), with features not
found in most of them
([working-copy-as-a-commit](https://martinvonz.github.io/jj/latest/working-copy),
[undo functionality](https://martinvonz.github.io/jj/latest/operation-log),
automatic rebase, [safe replication via `rsync`, Dropbox, or distributed file
system](https://martinvonz.github.io/jj/latest/technical/concurrency)).

The command-line tool is called `jj` for now because it's easy to type and easy
to replace (rare in English). The project is called "Jujutsu" because it matches
"jj".

Jujutsu is relatively young, with lots of work to still be done. If you have any
questions, or want to talk about future plans, please join us on Discord
[![Discord](https://img.shields.io/discord/968932220549103686.svg?label=&logo=discord&logoColor=ffffff&color=7389D8&labelColor=6A7EC2)](https://discord.gg/dkmfj3aGQN)
or start a [GitHub Discussion](https://github.com/martinvonz/jj/discussions); the
developers monitor both channels.

> [!IMPORTANT]
> Jujutsu is an **experimental version control system**. While Git compatibility
> is stable, and most developers use it daily for all their needs, there may
> still be work-in-progress features, suboptimal UX, and workflow gaps that make
> it unusable for your particular use.

### News and Updates 📣

- **Jan 2023**: Martin gave a presentation about Google's plans for Jujutsu at
  Git Merge 2022! See the
  [slides](https://docs.google.com/presentation/d/1F8j9_UOOSGUN9MvHxPZX_L4bQ9NMcYOp1isn17kTC_M/view)
  or the [recording](https://www.youtube.com/watch?v=bx_LGilOuE4).

## Getting started

Follow the [installation
instructions](https://martinvonz.github.io/jj/latest/install-and-setup) to
obtain and configure `jj`.

The best way to get started is probably to go through [the
tutorial](https://martinvonz.github.io/jj/latest/tutorial). Also see the [Git
comparison](https://martinvonz.github.io/jj/latest/git-comparison), which
includes a table of `jj` vs. `git` commands.

As you become more familiar with Jujutsu, the following resources may be helpful:

- The [FAQ](https://martinvonz.github.io/jj/latest/FAQ).
- The [Glossary](https://martinvonz.github.io/jj/latest/glossary).
- The `jj help` command (e.g. `jj help rebase`).

If you are using a **prerelease** version of `jj`, you would want to consult
[the docs for the prerelease (main branch)
version](https://martinvonz.github.io/jj/prerelease/). You can also get there
from the docs for the latest release by using the website's version switcher. The version switcher is visible in
the header of the website when you scroll to the top of any page.

## Features

### Compatible with Git

Jujutsu has two
[backends](https://martinvonz.github.io/jj/latest/glossary#backend). One of them
is a Git backend (the other is a native one [^native-backend]). This lets you
use Jujutsu as an alternative interface to Git. The commits you create will look
like regular Git commits. You can always switch back to Git. The Git support
uses the [libgit2](https://libgit2.org/) C library.

[^native-backend]: At this time, there's practically no reason to use the native
backend. The backend exists mainly to make sure that it's possible to eventually
add functionality that cannot easily be added to the Git backend.

<img src="demos/git_compat.png" />

You can even have a ["co-located" local
repository](https://martinvonz.github.io/jj/latest/git-compatibility#co-located-jujutsugit-repos)
where you can use both `jj` and `git` commands interchangeably.

### The working copy is automatically committed

Jujutsu uses a real commit to represent the working copy. Checking out a commit
results a new working-copy commit on top of the target commit. Almost all
commands automatically amend the working-copy commit.

The working-copy being a commit means that commands never fail because the
working copy is dirty (no "error: Your local changes to the following
files..."), and there is no need for `git stash`. Also, because the working copy
is a commit, commands work the same way on the working-copy commit as on any
other commit, so you can set the commit message before you're done with the
changes.

<img src="demos/working_copy.png" />

### The repo is the source of truth

With Jujutsu, the working copy plays a smaller role than with Git. Commands
snapshot the working copy before they start, then they update the repo, and then
the working copy is updated (if the working-copy commit was modified). Almost
all commands (even checkout!) operate on the commits in the repo, leaving the
common functionality of snapshotting and updating of the working copy to
centralized code. For example, `jj restore` (similar to `git restore`) can
restore from any commit and into any commit, and `jj describe` can set the
commit message of any commit (defaults to the working-copy commit).

### Entire repo is under version control

All operations you perform in the repo are recorded, along with a snapshot of
the repo state after the operation. This means that you can easily revert to an
earlier repo state, or to simply undo a particular operation (which does not
necessarily have to be the most recent operation).

<img src="demos/operation_log.png" />

### Conflicts can be recorded in commits

If an operation results in
[conflicts](https://martinvonz.github.io/jj/latest/glossary#conflict),
information about those conflicts will be recorded in the commit(s). The
operation will succeed. You can then resolve the conflicts later. One
consequence of this design is that there's no need to continue interrupted
operations. Instead, you get a single workflow for resolving conflicts,
regardless of which command caused them. This design also lets Jujutsu rebase
merge commits correctly (unlike both Git and Mercurial).

Basic conflict resolution:

<img src="demos/resolve_conflicts.png" />

Juggling conflicts:

<img src="demos/juggle_conflicts.png" />

### Automatic rebase

Whenever you modify a commit, any descendants of the old commit will be rebased
onto the new commit. Thanks to the conflict design described above, that can be
done even if there are conflicts. Branches pointing to rebased commits will be
updated. So will the working copy if it points to a rebased commit.

### Comprehensive support for rewriting history

Besides the usual rebase command, there's `jj describe` for editing the
description (commit message) of an arbitrary commit. There's also `jj diffedit`,
which lets you edit the changes in a commit without checking it out. To split
a commit into two, use `jj split`. You can even move part of the changes in a
commit to any other commit using `jj move`.

## Status

The tool is quite feature-complete, but some important features like (the
equivalent of) `git blame` are not yet supported. There
are also several performance bugs. It's also likely that workflows and setups
different from what the core developers use are not well supported.

I (Martin von Zweigbergk) have almost exclusively used `jj` to develop the
project itself since early January 2021. I haven't had to re-clone from source
(I don't think I've even had to restore from backup).

There *will* be changes to workflows and backward-incompatible changes to the
on-disk formats before version 1.0.0. Even the binary's name may change (i.e.
away from `jj`). For any format changes, we'll try to implement transparent
upgrades (as we've done with recent changes), or provide upgrade commands or
scripts if requested.

## Related work

There are several tools trying to solve similar problems as Jujutsu. See
[related work](https://martinvonz.github.io/jj/latest/related-work) for details.

## Contributing

We welcome outside contributions, and there's plenty of things to do, so
don't be shy. Please ask if you want a pointer on something you can help with,
and hopefully we can all figure something out.

We do have [a few policies and
suggestions](https://martinvonz.github.io/jj/prerelease/contributing/)
for contributors. The broad TL;DR:

- Bug reports are very welcome!
- Every commit that lands in the `main` branch is code reviewed.
- Please behave yourself, and obey the Community Guidelines.
- There **is** a mandatory CLA you must agree to. Importantly, it **does not**
  transfer copyright ownership to Google or anyone else; it simply gives us the
  right to safely redistribute and use your changes.

### Mandatory Google Disclaimer

I (Martin von Zweigbergk, <martinvonz@google.com>) started Jujutsu as a hobby
project in late 2019, and it has evolved into my full-time project at Google,
with several other Googlers (now) assisting development in various capacities.
That said, **this is not a Google product**.

## License

Jujutsu is available as Open Source Software, under the Apache 2.0 license. See
[LICENSE](./LICENSE) for details about copyright and redistribution.
