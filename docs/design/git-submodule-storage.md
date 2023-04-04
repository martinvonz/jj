# Git submodule storage

## Objective

Decide what approach(es) to Git submodule storage we should pursue.
The decision will be recorded in [./git-submodules.md](./git-submodules.md).

## Use cases to consider

The submodule storage format should support the workflows specified in the
[submodules roadmap](./git-submodules.md). It should be obvious how "Phase 1"
requirements will be supported, and we should have an idea of how "Phases 2,3,X"
might be supported.

Notable use cases and workflows are noted below.

### Fetching submodule commits

Git's protocol is designed for communicating between copies of the same
repository. Notably, a Git fetch calculates the list of required objects by
performing reachability checks between the refs on the local and the remote
side. We should expect that this will only work well if the submodule repository
is stored as a local Git repository.

Rolling our own Git fetch is too complex to be worth the effort.

### "jj op restore" and operation log format

We want `jj op restore` to restore to an "expected" state in the submodule.
There is a potential distinction between running `jj op restore` in the
superproject vs in the submodule, and the expected behavior may be different in
each case, e.g. in the superproject, it might be enough to restore the submodule
working copy, but in the submodule, refs also need to be restored.

Currently, the operation log only references objects and refs in the
superproject, so it is likely that proposed approaches will need to extend this
format. It is also worth considering that submodules may be added, updated or
removed in superproject commits, thus the list of submodules is likely to change
over the repository's lifetime.

### Nested submodules

Git submodules may contain submodules themselves, so our chosen storage schemes
should support that.

We should consider limiting the recursion depth to avoid nasty edge cases (e.g.
cyclical submodules.) that might surprise users.

### Supporting future extensions

There are certain extensions we may want to make in the future, but we don't
have a timeline for them today. Proposed approaches should take these
extensions into account (e.g. the approach should be theoretically extensible),
but a full proposal for implementing them is not necessary.

These extensions are:

- Non-git subrepos
- Colocated Git repos
- Non-git backends

## Possible approaches

### Approach 1: Store Git submodules as full jj repos

This would be somewhere in `.jj` but outside of `.jj/store`. We would then
expose a "submodules" interface that gets hooked up to the relevant machinery
(e.g. updating the working copy).

TODO(chooglen): Discuss operation log
TODO(chooglen): Discuss nested submodules

### Approach 3: Store Git submodules as alternate jj repo backends

This is Approach 3, but instead of storing the submodule in a Git backend,
create a new backend that is backed by a full jj repo (like Approach 2), and
store the Git submodule in its own jj repo backend.

TODO(chooglen): Discuss operation log
TODO(chooglen): Discuss nested submodules
