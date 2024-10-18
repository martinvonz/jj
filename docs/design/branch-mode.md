# Branch mode

This is a proposed configuration option to replace
`experimental-advance-branches`. It is not intended to replace or obviate
topics.

The design below assumes that the configuration option is enabled. There should
be no change from current behavior if the option is not enabled.

**"Non-headless"** or **"branch"** mode: a git-colocated repository with the
new configuration option enabled is considered to be in this mode whenever
`.git/HEAD` (hereafter `HEAD`) is populated with a valid `jj` bookmark, and the
bookmark is currently at `@` or `@-`. In this design, the bookmark will be
referred to as `b(HEAD)`.

**Note:** The only part of this design that is specific to `git` colocation is
the use of `.git/HEAD`. The design could be implemented for any backend that
has a concept of a "current" branch.

## Invariants

The configuration option will never affect any of these `jj` behaviors:

* how `@` is updated by each `jj` command
* the way commit topology or contents (ignoring bookmarks) evolve with each
  command

## Diagrams

These Excalidraw diagrams show the effect of various `jj` commands when in
branch mode:
[final rendering TBD; see PR-description for updated link]

In these diagrams, a colored arrow is a `jj` bookmark, and a bold and colored
commit-graph node is `b(HEAD)`.

## Entering branch mode

These commands always enter branch mode, by setting `.git/HEAD` appropriately:

```
git init
git clone
bookmark set <name> @
bookmark create <name> @
bookmark set <name> @-
bookmark create <name> @-
```

See also "Changing branches."

## Staying on the same Git branch

When in branch mode, these operations preserve branch mode, and do not change
which bookmark `HEAD` points to:

```
rebase
abandon
commit
new [without specifying revisions]
duplicate
backout
```

... as well as any commands that don't change the change topology, such as
`status`, `describe`, `diff`, etc.

In branch mode, each of the above commands acts on `b(HEAD)` the same way it
acts on `@`:
* when `@` is `b(HEAD)`, after the command, `b(HEAD)` is set to `@`.
* when `@-` is `b(HEAD)`, after the command, `b(HEAD)` is set to `@-`, unless
  that would be ambiguous (for instance, with `jj abandon @-` if the original
  `@-` has multiple parents); in case of ambiguity, `b(HEAD)` is set to `@`.

Note: I haven't thought of any cases where this would occur, but it may be that
there is an "obvious" resolution to some otherwise-ambiguous `@-` cases, in
which case we would not need to set `b(HEAD)` to `@`; for instance, if only one
commit in `@-` is a viable candidate for `jj branch set b(HEAD) <rev>` without
`--allow-backwards`, then we would pick that revision.

## Changing branches

These operations may either enter branch mode or change which branch is in
`HEAD`:

```
edit [bookmark]
new [bookmark]
```

With a revision argument that is *not* a bookmark (including `<branch>@git` or
`<branch>@origin`), these commands will always exit branch mode (i.e. leave git
in the "headless" state).

## Merges:

```
new [bookmark1] [bookmark2] ...
```

If none of the bookmarks is `b(HEAD)`, this will simply enter headless mode.

As long as any of the bookmarks is `b(HEAD)`, this will remain in branch mode,
but will *not* update the bookmark to the new commit (even if `@` is initially
`b(HEAD)`). That is, after this operation, `b(HEAD)` will be a member of `@-`.

This will ensure that the next `commit` or `new` (without a revision argument)
will behave as described above: only `b(HEAD)` will be advanced. This means
that, in Git terminology, to merge changes "from" branch (bookmark) A "into"
branch (bookmark) B, the user would first checkout `B` and then use `jj new A B
; jj new`. B would be advanced to the merge commit, while A would not.

## Diffing:

In non-headless mode, `jj diff` (with no revset) would default to `jj diff @
--from b(HEAD)`.
