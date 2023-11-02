# Comparison with Sapling

## Introduction

This document attempts to describe how jj is different
from [Sapling](https://sapling-scm.com). Sapling is a VCS developed by Meta. It is a
heavily modified fork of [Mercurial](https://www.mercurial-scm.org/). Because
jj has copied many ideas from Mercurial, there are many similarities between the
two tools, such as:

* A user-friendly CLI
* A "[revset](revsets.md)" language for selecting revisions
* Good support for working with stacked commits, including tracking "anonymous
  heads" (no "detached HEAD" state like in Git) and `split` commands, and
  automatically rebasing descendant commits when you amend a commit.
* Flexible customization of output using [templates](templates.md)

## Differences

Here is a list of some differences between jj and Sapling.

* **Working copy:** When using Sapling (like most VCSs), the
  user explicitly tells the tool when to create a commit and which files to
  include. When using jj, the working copy
  is [automatically snapshotted by every command](working-copy.md). New files
  are automatically tracked and deleted files are automatically untracked. This
  has several advantages:

  * The working copy is effectively backed up every time you run a command.
  * No commands fail because you have changes in the working copy ("abort: 1
    conflicting file changes: ..."). No need for `sl shelve`.
  * Simpler and more consistent CLI because the working copy is treated like any
    other commit.

* **Conflicts:** Like most VCSs, Sapling requires the user to
  resolve conflicts before committing. jj lets
  you [commit conflicts](conflicts.md). Note that it's a representation of the
  conflict that's committed, not conflict markers (`<<<<<<<` etc.). This also
  has several advantages:

  * Merge conflicts won't prevent you from checking out another commit.
  * You can resolve the conflicts when you feel like it.
  * Rebasing descendants always succeeds. Like jj, Sapling automatically
    rebases, but it will fail if there are conflicts.
  * Merge commits can be rebased correctly (Sapling sometimes fails).
  * You can rebase conflicts and conflict resolutions.

* **Undo:** jj's undo is powered by [the operation log](operation-log.md), which
  records how the repo has changed over time. Sapling has a similar feature
  with its [MetaLog](https://sapling-scm.com/docs/internals/metalog).
  They seem to provide similar functionality, but jj also exposes the log to the
  user via `jj op log`, so you can tell how far back you want to go back.
  Sapling has `sl debugmetalog`, but that seems to show the history of a single
  commit, not the whole repo's history. Thanks to jj snapshotting the working
  copy, it's possible to undo changes to the working copy. For example, if
  you `jj undo` a ` jj commit`, `jj diff` will show the same changes as
  before `jj commit`, but if you `sl undo` a `sl commit`, the working copy will
  be clean.
* **Git interop:** Sapling supports cloning, pushing, and pulling from a remote
  Git repo. jj also does, and it also supports sharing a working copy with a Git
  repo, so you can use `jj` and `git` interchangeably in the same repo.
* **Polish:** Sapling is much more polished and feature-complete. For example,
  jj has no `blame/annotate` or `bisect` commands, and also no copy/rename
  support. Sapling also has very nice web UI
  called [Interactive Smartlog](https://sapling-scm.com/docs/addons/isl), which
  lets you drag and drop commits to rebase them, among other things.
* **Forge workflow:** Sapling has `sl pr submit --stack`, which lets you
  push a stack of commits as separate GitHub PRs, including setting the base
  branch. It only supports GitHub. jj doesn't have any direct integration with
  GitHub or any other forge. However, it has `jj git push --change` for
  automatically creating branches for specified commits. You have to specify
  each commit you want to create a branch for by using
  `jj git push --change X --change Y ...`, and you have to manually set up any
  base branches in GitHub's UI (or GitLab's or ...). On subsequent pushes, you
  can update all at once by specifying something like `jj git push -r main..@`
  (to push all branches on the current stack of commits from where it forked
  from `main`).
