# First-class conflicts


## Introduction

Like [Pijul](https://pijul.org/) and [Darcs](http://darcs.net/) but unlike most
other VCSs, Jujutsu can record conflicted states in commits. For example, if you
rebase a commit and it results in a conflict, the conflict will be recorded in
the rebased commit and the rebase operation will succeed. You can then resolve
the conflict whenever you want. Conflicted states can be further rebased,
merged, or backed out. Note that what's stored in the commit is a logical
representation of the conflict, not conflict *markers*; rebasing a conflict
doesn't result in a nested conflict markers (see
[technical doc](technical/conflicts.md) for how this works).


## Advantages

The deeper understanding of conflicts has many advantages:

* Removes the need for things like
  `git rebase/merge/cherry-pick/etc --continue`. Instead, you get a single
  workflow for resolving conflicts: check out the conflicted commit, resolve
  conflicts, and amend.
* Enables the "auto-rebase" feature, where descendants of rewritten commits
  automatically get rewritten. This feature mostly replaces Mercurial's
  [Changeset Evolution](https://www.mercurial-scm.org/wiki/ChangesetEvolution).
* Lets us define the change in a merge commit as being compared to the merged
  parents. That way, we can rebase merge commits correctly (unlike both Git and
  Mercurial). That includes conflict resolutions done in the merge commit,
  addressing a common use case for
  [git rerere](https://git-scm.com/docs/git-rerere).
  Since the changes in a merge commit are displayed and rebased as expected,
  [evil merges](https://git-scm.com/docs/gitglossary/2.22.0#Documentation/gitglossary.txt-aiddefevilmergeaevilmerge)
  are arguably not as evil anymore.
* Allows you to postpone conflict resolution until you're ready for it. You
  can easily keep all your work-in-progress commits rebased onto upstream's head
  if you like.
* [Criss-cross merges](https://stackoverflow.com/questions/26370185/how-do-criss-cross-merges-arise-in-git)
  and [octopus merges](https://git-scm.com/docs/git-merge#Documentation/git-merge.txt-octopus)
  become trivial (implementation-wise); some cases that Git can't currently
  handle, or that would result in nested conflict markers, can be automatically
  resolved.
* Enables collaborative conflict resolution. (This assumes that you can share
  the conflicts with others, which you probably shouldn't do if some people
  interact with your project using Git.)

For information about how conflicts are handled in the working copy, see
[here](working-copy.md#conflicts).


## Conflict markers

Conflicts are "materialized" using *conflict markers* in various contexts. For
example, when you run `jj edit` on a commit with a conflict, it will be
materialized in the working copy. Conflicts are also materialized when they are
part of diff output (e.g. `jj show` on a commit that introduces or resolves a
conflict). Here's an example of how Git can render a conflict using [its "diff3"
style](https://git-scm.com/docs/git-merge#_how_conflicts_are_presented):

```
  <<<<<<< left
  apple
  grapefruit
  orange
  ======= base
  apple
  grape
  orange
  ||||||| right
  APPLE
  GRAPE
  ORANGE
  >>>>>>>
```

In this example, the left side changed "grape" to "grapefruit", and the right
side made all lines uppercase. To resolve the conflict, we would presumably keep
the right side (the third section) and replace "GRAPE" by "GRAPEFRUIT". This way
of visually finding the changes between the base and one side and then applying
them to the other side is a common way of resolving conflicts when using Git's
"diff3" style.

Jujutsu helps you by combining the base and one side into a unified diff for
you, making it easier to spot the differences to apply to the other side. Here's
how that would look for the same example as above:

```
  <<<<<<<
  %%%%%%%
   apple
  -grape
  +grapefruit
   orange
  +++++++
  APPLE
  GRAPE
  ORANGE
  >>>>>>>
```

As in Git, the `<<<<<<<` and `>>>>>>>` lines mark the start and end of the
conflict. The `%%%%%%%` line indicates the start of a diff. The `+++++++`
line indicates the start of a snapshot (not a diff).

There is another reason for this format (in addition to helping you spot the
differences): The format supports more complex conflicts involving more than 3
inputs. Such conflicts can arise when you merge more than 2 commits. They would
typically be rendered as a single snapshot (as above) but with more than one
unified diffs. The process for resolving them is similar: Manually apply each
diff onto the snapshot.
