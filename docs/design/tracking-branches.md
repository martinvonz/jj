# Remote/`@git` tracking branches

This is a plan to implement more Git-like remote tracking branch UX.

## Objective

`jj` imports all remote branches to local branches by default. As described in
[#1136], this doesn't interact nicely with Git if we have multiple Git remotes
with a number of branches. The `git.auto-local-branch` config can mitigate this
problem, but we'll get locally-deleted branches instead.

The goal of this plan is to implement
* proper support for tracking/non-tracking remote branches
* logically consistent data model for importing/exporting Git refs

[#1136]: https://github.com/martinvonz/jj/issues/1136

## Current data model (as of jj 0.8.0)

Under the current model, all remote branches are "tracking" branches, and
remote changes are merged into the local counterparts.

```
branches
  [name]:
    local_target?
    remote_targets[remote]: target
tags
  [name]: target
git_refs
  ["refs/heads/{name}"]: target             # last-known local branches
  ["refs/remotes/{remote}/{name}"]: target  # last-known remote branches
                                            # (copied to remote_targets)
  ["refs/tags/{name}"]: target              # last-known tags
git_head: target?
```

* Remote branches are stored in both `branches[name].remote_targets` and
  `git_refs["refs/remotes"]`. These two are kept in sync unless the branch is
  removed by `jj branch forget` command.
* Pseudo `@git` remote branches are stored in `git_refs["refs/heads"]`.

## Proposed data model

We'll add a per-remote-branch `state` to distinguish non-tracking branches
from tracking ones.

```
state = new        # not merged in the local branch or tag
      | tracking   # merged in the local branch or tag
      | forgotten  # to be expunged on the next export
# `ignored` state could be added if we want to manage it by view, not by
# config file. target of ignored remote branch would be absent.
```

We'll add a per-remote view-like object to record the last known remote
branches. It will replace `git_refs` and `branches[name].remote_targets` in
the current model.

```
branches
  [name]: target
tags
  [name]: target
remotes
  ["git"]:
    branches
      [name]: target, state                 # refs/heads/{name}
    tags
      [name]: target, state = tracking      # refs/tags/{name}
    head: target?, state = TBD              # refs/HEAD
  [remote]:
    branches
      [name]: target, state                 # refs/remotes/{remote}/{name}
    tags: (empty)
    head: (empty)
```

With the proposed data model, we can
* naturally support remote branches which have no local counterparts
* deduplicate `branches[name].remote_targets` and `git_refs["refs/remotes"]`
* eliminate `git_` variables and methods from the view object

The `git.auto-local-branch` config knob is applied when importing new remote
branch. `jj branch` sub commands will be added to change the tracking state.

```rust
fn default_state_for_newly_imported_branch(config, remote) {
    if remote == "git" {
        State::Tracking
    } else if config["git.auto-local-branch"] {
        State::Tracking
    } else {
        State::New
    }
}
```

A branch target to be merged is calculated based on the `state`.

```rust
fn target_in_merge_context(known_target, state) {
    match state {
        State::New => RefTarget::absent(),
        State::Tracking => known_target,
        State::Forgotten => RefTarget::absent(),
    }
}
```

### Mapping to the current data model

* New `remotes["git"].branches` corresponds to `git_refs["refs/heads"]`.
* New `remotes["git"].tags` corresponds to `git_refs["refs/tags"]`.
* New `remotes["git"].head` corresponds to `git_head`.
* New `remotes[remote].branches` corresponds to
  `git_refs["refs/remotes/{remote}"]` and `branches[].remote_targets[remote]`.
* If `git_refs["refs/remotes/{remote}"]` exists but `.remote_targets` doesn't,
  it means `state = forgotten` in new model.
* `state = new|tracking` doesn't exist in the current model. It's determined
  by `git.auto-local-branch` config.

## Common command behaviors

In the following sections, a merge is expressed as `adds - removes`.
In particular, a merge of local and remote targets is
`[local, remote] - [known_remote]`.

### fetch/import

* `jj git fetch`
  1. Fetches remote changes to the backing Git repo.
  2. Import changes only for `remotes[remote].branches[glob]` (see below)
     * TODO: how about fetched `.tags`?

* `jj git import`
  1. Calculates diff from the known `remotes` to the actual git repo.
     * `"refs/heads" - remotes["git"].branches`
     * `"refs/tags" - remotes["git"].tags`
     * `"HEAD" - remotes["git"].head` (unused)
     * `"refs/remotes/{remote}" - remotes[remote]`
  2. Merges diff in local `branches` and `tags` if `state` is `tracking`.
     * If the branch is new, the default `state` should be calculated.
     * If `state` is `forgotten`, the known branch is supposed to be removed,
       and the default `state` should be calculated.
  3. Updates `remotes` reflecting the import.
     * `absent` entries are removed from `remotes`.
  4. Abandons commits that are no longer referenced.

### push/export

* `jj git push`
  1. Calculates diff from the known `remotes[remote]` to the local changes.
     * `branches - remotes[remote].branches`
       * If `state` is `new|forgotten` (i.e. untracked), the known remote
         branch `target` is considered `absent`.
       * If `state` is `new|forgotten`, and if the local branch `target` is
         `absent`, the diff `[absent, remote] - absent` is noop. So it's not
         allowed to push deleted branch to untracked remote.
       * TODO: Copy Git's `--force-with-lease` behavior?
     * ~`tags`~ (not implemented, but should be the same as `branches`)
  2. Pushes diff to the remote Git repo (as well as remote tracking branches
     in the backing Git repo.)
  3. Sets `remotes[remote].branches[name].state = tracking`
  4. Import changes only for `remotes[remote].branches[glob]`

* `jj git export`
  1. Calculates diff from the known `remotes["git"]` to the local changes
     and forgotten branches.
     * `branches - remotes["git"].branches` if `state` is `tracking`
       * If `remotes["git"].branches[name]` is `absent`, the default
         `state = tracking` applies.
       * If `state` is `forgotten` but local branch exists,
         `remotes["git"].branches[name]` is supposed to be removed, and
         the default `state = tracking` applies.
     * ~`tags`~ (not implemented, but should be the same as `branches`)
     * `absent - remotes[remote].branches` if `state` is `forgotten`
  2. Applies diff to the backing Git repo.
  3. Updates `remotes` reflecting the export.
     * `absent` entries are removed from `remotes`.

### init/clone

* `jj init`
  * Import, track, and merge per `git.auto_local_branch` config.
  * If `!git.auto_local_branch`, no `tracking` state will be set.

* `jj git clone`
  * Import, track, and merge per `git.auto_local_branch` config.
  * The default branch will be tracked regardless of `git.auto_local_branch`
    config. (Because local branch is created for the default remote branch,
    it makes sense to track.)

### branch

* `jj branch set {name}`
  1. Sets local `branches[name]` entry.
* `jj branch delete {name}`
  1. Removes local `branches[name]` entry.
* `jj branch forget {name}`
  1. Removes local `branches[name]` entry if exists.
  2. Sets all `remotes[remote].branches[name].state = forgotten`.
* `jj branch track {name}@{remote}` (new command)
  1. Merges `[local, remote] - [absent]` in local branch.
     * Same as "fetching/importing existing branch from untracked remote".
  2. Sets `remotes[remote].branches[name].state = tracking`.
* `jj branch untrack {name}@{remote}` (new command)
  1. Sets `remotes[remote].branches[name].state = new`.
* `jj branch list`
  * TODO: hide non-tracking branches by default? ...

Note: desired behavior of `jj branch forget` is to
* discard both local and remote branches (without actually removing branches
  at remotes)
* not abandon commits which belongs to those branches (even if the branch is
  removed at a remote)

## Command behavior examples

### fetch/import

* Fetching/importing new branch
  1. Decides new `state = new|tracking` based on `git.auto_local_branch`
  2. If new `state` is `tracking`, merges `[absent, new_remote] - [absent]`
     (i.e. creates local branch with `new_remote` target)
  3. Sets `remotes[remote].branches[name].state`
* Fetching/importing existing branch from tracking remote
  1. Merges `[local, new_remote] - [known_remote]`
* Fetching/importing existing branch from untracked remote
  1. Decides new `state = new|tracking` based on `git.auto_local_branch`
  2. If new `state` is `tracking`, merges `[local, new_remote] - [absent]`
  3. Sets `remotes[remote].branches[name].state`
* Fetching/importing remotely-deleted branch from tracking remote
  1. Merges `[local, absent] - [known_remote]`
  2. Removes `remotes[remote].branches[name]` (`target` becomes `absent`)
     (i.e. the remote branch is no longer tracked)
  3. Abandons commits in the deleted branch
* Fetching/importing remotely-deleted branch from untracked remote
  1. Decides new `state = new|tracking` based on `git.auto_local_branch`
  2. Noop anyway since `[local, absent] - [absent]` -> `local`
* Fetching previously-forgotten branch from remote
  1. Decides new `state = new|tracking` based on `git.auto_local_branch`
  2. If new `state` is `tracking`, merges
    `[absent, new_remote] - [absent]` -> `new_remote`
     (The known `target` of forgotten remote branch is `absent`)
  3. Sets `remotes[remote].branches[name].state`
* Fetching forgotten and remotely-deleted branch
  * Same as "remotely-deleted branch from untracked remote" since `forgotten`
    remote branch should never be `tracking`
  * Therefore, no local commits should be abandoned

### push/export

* Pushing/exporting new branch, remote doesn't exist
  1. Exports `[local, absent] - [absent]` -> `local`
  2. Sets `remotes[remote].branches[name].state = tracking`
  3. `import_refs()` merges `[local, local] - [absent]` -> `local` (noop)
* Pushing/exporting new branch, untracked remote exists
  1. Exports `[local, remote] - [absent]`
     * Fails if `local` moved backwards or sideways
  2. Sets `remotes[remote].branches[name].state = tracking`
  3. `import_refs()` merges `[local, local] - [remote]` -> `local` (noop)
* Pushing/exporting existing branch to tracking remote
  1. Exports `[local, remote] - [remote]` -> `local`
     * Fails if `local` moved backwards or sideways, and if `remote` is out of
       sync
  2. `import_refs()` merges `[local, local] - [remote]` -> `local` (noop)
* Pushing/exporting existing branch to untracked remote
  * Same as "new branch"
* Pushing/exporting deleted branch to tracking remote
  1. Exports `[absent, remote] - [remote]` -> `absent`
     * TODO: Fails if `remote` is out of sync?
  2. `import_refs()` merges `[absent, absent] - [remote]` -> `absent`
  3. Removes `remotes[remote].branches[name]` (`target` becomes `absent`)
* Pushing/exporting deleted branch to untracked remote
  * Noop since `[absent, remote] - [absent]` -> `remote`
  * Perhaps, UI will report error
* Pushing forgotten branch to untracked remote
  * Same as "deleted branch to untracked remote"
* Exporting forgotten branch
  1. Local branch change is noop since `[absent, absent] - [absent]` -> `absent`
  2. Exports `forgotten` state to the backing Git repo:
    `[absent, known_remote] - [known_remote]` -> `absent`
    (This includes local branch in the pseudo `"git"` remote)
  3. Removes `remotes[remote].branches[name]` (`target` becomes `absent`)
* Pushing previously-forgotten branch to remote
  * Same as "new branch, untracked remote exists"
  * The known `target` of forgotten remote branch is `absent`

### `@git` remote

* `jj branch untrack {name}@git`
  * Maybe rejected (to avoid confusion)?
  * Allowing this would mean different local branches of the same name coexist
    in jj and git.
* `jj git fetch --remote git`
  * Maybe rejected (to avoid confusion)?
  * Conceptually, it's `git::import_refs()` only for local branches.
* `jj git push --remote git`
  * Maybe rejected (to avoid confusion)?
  * Conceptually, it's `jj branch track` and `git::export_refs()` only for
    local branches.

## Remaining issues

* `git.auto_local_branch = false` by default to help Git interop?
  * https://github.com/martinvonz/jj/issues/1862
* https://github.com/martinvonz/jj/issues/1278 pushing to tracked remote
  * Option could be added to push to all `tracking` remotes?
* Track remote branch locally with different name
  * Local branch name could be stored per remote branch
  * Consider UI complexity
* "private" state (suggested by @ilyagr)
  * "private" branches can be pushed to their own remote, but not to the
    upstream repo
  * This might be a state attached to a local branch (similar to Mercurial's
    "secret" phase)

## References

* https://github.com/martinvonz/jj/issues/1136
* https://github.com/martinvonz/jj/issues/1666
* https://github.com/martinvonz/jj/issues/1690
* https://github.com/martinvonz/jj/issues/1734
* https://github.com/martinvonz/jj/pull/1739
