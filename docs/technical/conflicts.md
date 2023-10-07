# First-class conflicts

## Introduction

Conflicts can happen when two changes are applied to some state. This document
is about conflicts between changes to files (not about [conflicts between
changes to branch targets](concurrency.md), for example).

For example, if you merge two branches in a repo, there may be conflicting
changes between the two branches. Most DVCSs require you to resolve those
conflicts before you can finish the merge operation. Jujutsu instead records
the conflicts in the commit and lets you resolve the conflict when you feel like
it.

## Data model

When a merge conflict happens, it is recorded as an ordered list of tree objects
linked from the commit (instead of the usual single tree per commit). There will
always be an odd number of trees linked from the commit. You can think of the
first tree as a start tree, and the subsequent pairs of trees to apply the diff
between onto the start. Examples:

* If the commit has trees A, B, C, D, and E it means that the contents should be
  calculated as A+(C-B)+(E-D).
* A three-way merge between A and C with B as base can be represented as a
commit with trees A, B, and C, also known as A+(C-B).

The resulting tree contents is calculated on demand. Note that we often don't
need to merge the entire tree. For example, when checking out a commit in the
working copy, we only need to merge parts of the tree that differs from the
tree that was previously checked out in the working copy. As another example,
when listing paths with conflicts, we only need to traverse parts of the tree
that cannot be trivially resolved; if only one side modified `lib/`, then we
don't need to look for conflicts in that sub-tree.

When merging trees, if we can't resolve a sub-tree conflict trivially by looking
at just the tree id, we recurse into the sub-tree. Similarly, if we can't
resolve a file conflict trivially by looking at just the id, we recursive into
the hunks within the file.

See [here](../git-compatibility.md#format-mapping-details) for how conflicts are
stored when using the Git commit backend.

## Conflict simplification

Remember that a 3-way merge can be written `A+C-B`. If one of those states is
itself a conflict, then we simply insert the conflict expression there. Then we
simplify by removing canceling terms. These two steps are implemented in
`Merge::flatten()` and `Merge::simplify()` in [`merge.rs`][merge-rs].

For example, let's say commit B is based on A and is rebased to C, where it
results in conflicts (`B+C-A`), which the user leaves unresolved. If the commit
is then rebased to D, the result will be `(B+C-A)+(D-C)` (`D-C` comes from
changing the base from C to D). That expression can be simplified to `B+D-A`,
which is a regular 3-way merge between B and D with A as base (no trace of C).
This is what lets the user keep old commits rebased to head without resolving
conflicts and still not get messy recursive conflicts.

As another example, let's go through what happens when you back out a conflicted
commit. Let's say we have the usual `B+C-A` conflict on top of non-conflict
state C. We then back out that change. Backing out ("reverting" in Git-speak) a
change means applying its reverse diff, so the result is `(B+C-A)+(A-(B+C-A))`,
which we can simplify to just `A` (i.e. no conflict).

[merge-rs]: https://github.com/martinvonz/jj/blob/main/lib/src/merge.rs
