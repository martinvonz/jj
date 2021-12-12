# First-class conflicts


## Introduction

Like [Pijul](https://pijul.org/) and [Darcs](http://darcs.net/) but unlike most
other VCSs, Jujutsu can record conflicted states in commits. For example, if you
rebase a commit and it results in a conflict, the conflict will be recorded in
the rebased commit and the rebase operation will succeed. You can then resolve
the conflict whenever you want. Conflicted states can be further rebased,
merged, or backed out. Note that what's stored in the commit is a logical
representation of the conflict, not conflict *markers*; rebasing a conflict
doesn't result in a nested conflict markers.


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
[here](working_copy.md#conflicts).
