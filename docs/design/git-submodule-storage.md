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
- The superproject using a non-git backend

## Proposed design

Git submodules will be stored as full jj repos. In the code, jj commands will
only interact with the submodule's repo as an entire unit, e.g. it cannot query
the submodule's commit backend directly. A well-abstracted submodule will extend
well to non-git backends and non-git subrepos.

The main challenge with this approach is that the submodule repo can be in a
state that is internally valid (when considering only the submodule's repo), but
invalid when considering the superproject-submodule system. This will be managed
by requiring all submodule interactions go through the superproject so that
superproject-submodule coordination can occur. For example, jj will not allow
the user to work on the submodule's repo without going through the superproject
(unlike Git).

The notable workflows could be addressed like so:

### Fetching submodule commits

The submodule would fetch using the equivalent of `jj git fetch`. It remains to
be decided how a "recursive" fetch should work, especially if a newly fetched
superproject commit references an unfetched submodule commit. A reasonable
approximation would be to fetch all branches in the submodule, and then, if the
submodule commit is still missing, gracefully handle it.

### "jj op restore" and operation log format

As full repos, each submodule will have its own operation log. We will continue
to use the existing operation log format, where each operation log tracks their
own repo's commits. As commands are run in the superproject, corresponding
commands will be run in the submodule as necessary, e.g. checking out a
superproject commit will cause a submodule commit to also be checked out.

Since there is no association between a superproject operation and a submodule
operation, `jj op restore` in the superproject will not restore the submodule to
a previous operation. Instead, the appropriate submodule operation(s) will be
created. This is sufficient to preserve the superproject-submodule relationship;
it precludes "recursive" restore (e.g. restoring branches in the superproject
and submodules) but it seems unlikely that we will need such a thing.

### Nested submodules

Since submodules are full repos, they can contain submodules themselves. Nesting
is unlikely to complicate any of the core features, since the top-level
superproject/submodule relationship is almost identical to the submodule/nested
submodule relationship.

### Extending to colocated Git repos

Git expects submodules to be in `.git/modules`, so it will not understand this
storage format. To support colocated Git repos, we will have to change Git to
allow a submodule's gitdir to be in an alternate location (e.g. we could add a
new `submodule.<name>.gitdir` config option). This is a simple change, so it
should be feasible.

## Alternatives considered

### Git repos in the main Git backend

Since the Git backend contains a Git repository, an 'obvious' default would be
to store them in the Git superproject the same way Git does, i.e. in
`.git/modules`. Since Git submodules are full repositories that can have
submodules, this storage scheme naturally extends to nested submodules.

Most of the work in storing submodules and querying them would be well-isolated
to the Git backend, which gives us a lot of flexibility to make changes without
affecting the rest of jj. However, the operation log will need a significant
rework since it isn't designed to reference submodules, and handling edge cases
(e.g. a submodule being added/removed, nested submodules) will be tricky.

This is rejected because handling that operation log complexity isn't worth it
when very little of the work extends to non-Git backends.

### Store Git submodules as alternate Git backends

Teach jj to use multiple commit backends and store Git submodules as Git
backends. Since submodules are separate from the 'main' backend, a repository
can use whatever backend it wants as its 'main' one, while still having Git
submodules in the 'alternate' Git backends.

This approach extends fairly well to non-Git submodules (which would be stored
in non-Git commit backends). However, this requires significantly reworking the
operation log to account for multiple commit backends. It is also not clear how
nested submodules will be supported since there isn't an obvious way to
represent a nested submodule's relationship to its superproject.
