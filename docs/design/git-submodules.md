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

TODO

### Storing submodules

Possible approaches under discussion. See
[./git-submodule-storage.md](./git-submodule-storage.md).

### Snapshotting new submodule changes

TODO

### Merging/rebasing with submodules

TODO
