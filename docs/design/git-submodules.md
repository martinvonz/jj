# Git submodules

This is an aspirational document that describes how jj _will_ support Git
submodules. Readers are assumed to have some familiarity with Git and Git
submodules.

This document is a work in progress; submodules are a big feature, and relevant
details will be filled in incrementally.

## Objective

This proposal aims to replicate the workflows users are used to with Git
submodules, e.g.:

- Cloning submodules
- Making new submodule commits and updating the superproject
- Fetching and pushing updates to the submodule's remote
- Viewing submodule history

When it is convenient, this proposal will also aim to make submodules easier to
use than Git's implementation.

### Non-goals

- Non-Git 'submodules' (e.g. native jj submodules, other VCSes)
- Non-Git backends (e.g. Google internal backend)
- Changing how Git submodules are implemented in Git

## Background

We mainly want to support Git submodules for feature parity, since Git
submodules are a standard feature in Git and are popular enough that we have
received user requests for them. Secondarily (and distantly so), Git submodules
are notoriously difficult to use, so there is an opportunity to improve the UX
over Git's implementation.

### Intro to Git Submodules

[Git submodules](https://git-scm.com/docs/gitsubmodules) are a feature of Git
that allow a repository (submodule) to be embedded inside another repository
(the superproject). Notably, a submodule is a full repository, complete with its
own index, object store and ref store. It can be interacted with like any other
repository, regardless of the superproject.

In a superproject commit, submodule information is captured in two places:

- A `gitlink` entry in the commit's tree, where the value of the `gitlink` entry
  is the submodule commit id. This tells Git what to populate in the working
  tree.

- A top level `.gitmodules` file. This file is in Git's config syntax and
  entries take the form `submodule.<submodule-name>.*`. These include many
  settings about the submodules, but most importantly:

  - `submodule<submodule-name>.path` contains the path from the root of the tree
    to the `gitlink` being described.

  - `submodule<submodule-name>.url` contains the url to clone the submodule
    from.

In the working tree, Git notices the presence of a submodule by the `.git` entry
(signifying the root of a Git repository working tree). This is either the
submodule's actual Git directory (an "old-form" submodule), or a `.git` file
pointing to `<superproject-git-directory>/modules/<submodule-name>`. The latter
is sometimes called the "absorbed form", and is Git's preferred mode of
operation.

## Roadmap

Git submodules should be implemented in an order that supports an increasing set
of workflows, with the goal of getting feedback early and often. When support is
incomplete, jj should not crash, but instead provide fallback behavior and warn
the user where needed.

The goal is to land good support for pure Jujutsu repositories, while colocated
repositories will be supported when convenient.

This section should be treated as a set of guidelines, not a strict order of
work.

### Phase 1: Readonly submodules

This includes work that inspects submodule contents but does not create new
objects in the submodule. This requires a way to store submodules in a jj
repository that supports readonly operations.

#### Outcomes

- Submodules can be cloned anew
- New submodule commits can be fetched
- Submodule history and branches can be viewed
- Submodule contents are populated in the working copy
- Superproject gitlink can be updated to an existing submodule commit
- Conflicts in the superproject gitlink can be resolved to an existing submodule
  commit

### Phase 2: Snapshotting new changes

This allows a user to write new contents to a submodule and its remote.

#### Outcomes

- Changes in the working copy can be recorded in a submodule commit
- Submodule branches can be modified
- Submodules and their branches can be pushed to their remote

### Phase 3: Merging/rebasing/conflicts

This allows merging and rebasing of superproject commits in a content-aware way
(in contrast to Git, where only the gitlink commit ids are compared), as well as
workflows that make resolving conflicts easy and sensible.

This can be done in tandem with Phase 2, but will likely require a significant
amount of design work on its own.

#### Outcomes

- Merged/rebased submodules result in merged/rebased working copy content
- Merged/rebased working copy content can be committed, possibly by creating
  sensible merged/rebased submodule commits
- Merge/rebase between submodule and non-submodule gives a sensible result
- Merge/rebase between submodule A and submodule B gives a sensible result

### Phase ?: An ideal world

I.e. outcomes we would like to see if there were no constraints whatsoever.

- Rewriting submodule commits rewrites descendants correctly and updates
  superproject gitlinks.
- Submodule conflicts automatically resolve to the 'correct' submodule commits,
  e.g. a merge between superproject commits creating a merge of the submodule
  commits.
- Nested submodules are as easy to work with as non-nested submodules.
- The operation log captures changes in the submodule.

## Design

### Guiding principles

These guiding principles exist to make it easy for us to create a coherent user
experience, and for users to understand how submodules work, especially when
they diverge from Git's implementation.

#### Submodules are not standalone repositories

Treating the submodule as a standalone jj repository is both detrimental and
unnecessary. The reasons why are easiest understood in comparison to Git, where
submodules _are_ standalone Git repositories:

- As a standalone repository, it is possible to bypass the superproject to
  interact with a submodule. This means that properties that the superproject
  relies on may be invalidated unexpectedly.

  For example, when `git gc` attempts to delete 'unreachable' objects, it takes
  into account the objects reachable by refs in the repository. However, in a
  submodule, it is possible that a commit is not reachable by a submodule ref,
  but it is reachable by a superproject commit. Without knowledge of the
  superproject, the submodule may delete objects that the superproject is
  relying on ([see relevant StackOverflow
  question](https://stackoverflow.com/questions/31640270/will-git-garbage-collect-commit-in-submodule-referred-to-by-a-top-level-reposito)).

- With Git submodules, 'the repository' changes based on where `git` is run in
  the working tree, which is a source of
  [confusion](https://github.com/martinvonz/jj/issues/494#issuecomment-1404338917).

We should keep in mind that a submodule exists to be integrated with a
superproject, otherwise, the submodule could just be an independent clone. As
such, jj should enforce tight integrations between superproject and submodule
by requiring that all interactions with the submodule must be initiated
from the superproject.

#### Commands should involve submodules by default

Submodules should be part of the 'regular' jj workflow. Users shouldn't have to
remember to tell jj to consider submodules and manual submodule management
should be reserved only for very exceptional cases. For example,

- `jj git clone` should clone a 'reasonable' set of submodules.

- Updating the working copy in the superproject should also update the submodule
  working copies.

Contrast these with Git, where the `--recurse-submodules` needs to be explicitly
passed, and is a constant source of confusion for users.

In exceptional cases, submodules might be excluded (e.g. a submodule remote
deletes the commits we rely on). We should anticipate these problems, recover
gracefully, and be explicit to the user.

#### Submodules are globally managed

In a regular jj or Git repository, objects are reusable because they are stored
in a database independent of the working copy. Similarly, we should manage
submodules globally and treat that as the source of truth; the working copy
should, at best, be treated as a hint.

A consequence of this is that it is expected that the working copy may be out
of sync with the global submodules, e.g. submodules may be missing, their
commits may not be fetched, and they may have different configurations at
different points in history. This is different from Git, which tends to
assume that both are always in sync, and behaves badly when they are not. We
should be prepared to handle missing submodules/submodule commits.

For this to make sense to users, we must make it easy to manage the global
submodules and to reconcile them with the working copy submodules. Examples of
this include: prompting the user to update submodules when the working copy
changes, reporting when the working copy is out out of sync, letting users
sync the submodule store to `.gitmodules`, and letting users perform manual CRUD
on submodules.

### Storing submodules

Possible approaches under discussion. See
[./git-submodule-storage.md](./git-submodule-storage.md).

### Snapshotting new submodule changes

TODO

### Merging/rebasing with submodules

TODO
