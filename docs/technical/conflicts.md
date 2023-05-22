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

When a merge conflict happens, it is recorded within the tree object as a
special conflict object (not a file object with conflict markers). Conflicts
are stored as a lists of states to add and another list of states to remove. A
"state" here can be a normal file, a symlink, or a tree. These two lists
together can be a viewed as a simple algebraic expression of positive and
negative terms. The order of terms is undefined.

For example, a regular 3-way merge between B and C, with A as base, is `B+C-A`
(`{ removes=[A], adds=[B,C] }`). A modify/remove conflict is `B-A`. An add/add
conflict is `B+C`. An octopus merge of N commits usually has N positive terms
and N-1 negative terms. A non-conflict state A is equivalent to a conflict state
containing just the term `A`. An empty expression indicates absence of any
content at that path. A conflict can thus encode a superset of what can be
encoded in a regular path state.


## Conflict simplification

Remember that a 3-way merge can be written `B+C-A`. If one of those states is
itself a conflict, then we simply insert the conflict expression there. Then we
simplify by removing canceling terms.

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
