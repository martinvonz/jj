# Frequently asked questions

### Why does my branch not move to the new commit after `jj new/commit`?

If you're familiar with Git, you might expect the current branch to move forward
when you commit. However, Jujutsu does not have a concept of a "current branch".

To move branches, use `jj branch set`.

### I made a commit and `jj git push --all` says "Nothing changed" instead of pushing it. What do I do?

`jj git push --all` pushes all _branches_, not all revisions. You have two
options:

* Using `jj git push --change` will automatically create a branch and push it.
* Using `jj branch` commands to create or move a branch to either the commit
  you want to push or a descendant on it. Unlike Git, Jujutsu doesn't do this
  automatically (see previous question).

### Where is my commit, why is it not visible in `jj log`?

Is your commit visible with `jj log -r 'all()'`?

If yes, you should be aware that `jj log` only shows the revisions matching
`revsets.log` by default. You can change it as described in [config] to show
more revisions.

If not, the revision may have been abandoned (e.g. because you
used `jj abandon`, or because it's an obsolete version that's been rewritten
with `jj rebase`, `jj describe`, etc). In that case, `jj log -r commit_id`
should show the revision as "hidden". `jj new commit_id` should make the
revision visible again.

See [revsets] and [templates] for further guidance.

### `jj` is said to record the working after `jj log` and every other command. Where can I see these automatic "saves"?  

Indeed, every `jj` command updates the current "working-copy" revision, marked 
with `@` in `jj log`. You can notice this by how the [commit ID] of the
working copy revision changes when it's updated. Note that, unless you move to
another revision (with `jj new` or `jj edit`, for example), the [change ID] will 
not change.

If you expected to see a historical view of your working-copy changes in 
`jj log`, as a chain in a parent-child relationship, this is not the case. 
Instead, each commit gets amended and the commit ID changes.

You can see the history of these changes using `jj obslog`. This will show the 
history of the commits that were previously the "working-copy commit", since 
the last time the change id of the working copy commit changed. The obsolete 
changes will be marked as "hidden". They are still accessible with any `jj` 
command (`jj diff`, for example), but you will need to use the commit id to 
refer to hidden commits.

You can also use `jj obslog -r` on revisions that were previously the 
working-copy revisions. Use `jj obslog -p` as an easy way to see a commit's 
evolution.

### Can I prevent Jujutsu from recording my unfinished work? I'm not ready to commit it.

Jujutsu automatically records new files in the current working-copy commit and
doesn't provide a way to prevent that.

However, you can easily record intermediate drafts of your work. If you think
you might want to go back to the current state of the working-copy commit,
simply use `jj new`. There's no need for the commit to be "finished" or even
have a description.

Then future edits will go into a new working-copy commit on top of the now
former working-copy commit. Whenever you are happy with another set of edits,
use `jj squash` to amend the previous commit.

For more options see the next question.

### Can I interactively create a new commit from only some of the changes in the working copy, like `git add -p && git commit` or `hg commit -i`?

Since the changes are already in the working-copy commit, the equivalent to
`git add -p && git commit`/`git commit -p`/`hg commit -i` is to split the
working-copy commit with `jj split -i` (or the practically identical
`jj commit -i`).

For the equivalent of `git commit --amend -p`/`hg amend -i`, use `jj squash -i`.

### Is there something like `git rebase --interactive` or `hg histedit`?

Not yet, you can check [this issue] for updates.

To reorder commits, it is for now recommended to rebase commits individually,
which may require multiple invocations of `jj rebase -r` or `jj rebase -s`.

To squash or split commits, use `jj squash` and `jj split`.

### How can I keep my scratch files in the repository?

You can keep your notes and other scratch files in the repository, if you add
a wildcard pattern to either the repo's `gitignore` or your global `gitignore`.
Something like `*.scratch` or `*.scratchpad` should do, after that rename the
files you want to keep around to match the pattern.

If `$EDITOR` integration is important, something like `scratchpad.*` may be more
helpful, as you can keep the filename extension intact (it
matches `scratchpad.md`, `scratchpad.rs` and more).

You can find more details on `gitignore` files [here][gitignore].

### How can I keep local changes around, but not use them for Pull Requests?

In general, you should separate out the changes to their own commit (using
e.g. `jj split`). After that, one possible workflow is to rebase your pending
PRs on top of the commit with the local changes. Then, just before pushing to a
remote, use `jj rebase -s child_of_commit_with_local_changes -d main` to move
the PRs back on top of `main`.

If you have several PRs, you can
try `jj rebase -s all:commit_with_local_changes+ -d main`
(note the `+`) to move them all at once.

An alternative workflow would be to rebase the commit with local changes on
top of the PR you're working on and then do `jj new commit_with_local_changes`.
You'll then need to use `jj new --before` to create new commits
and `jj move --to`
to move new changes into the correct commits.

### I accidentally amended the working copy. How do I move the new changes into its own commit?

Use `jj obslog -p` to see how your working-copy commit has evolved. Find the
commit you want to restore the contents to. Let's say the current commit (with
the changes intended for a new commit) are in commit X and the state you wanted
is in commit Y. Note the commit id (normally in blue at the end of the line in
the log output) of each of them. Now use `jj new` to create a new working-copy
commit, then run `jj restore --from Y --to @-` to restore the parent commit
to the old state, and `jj restore --from X` to restore the new working-copy
commit to the new state.

### How do I deal with divergent changes ('??' after the [change ID])?

A [divergent change][glossary_divergent_change] represents a change that has two
or more visible commits associated with it. To refer to such commits, you must
use their [commit ID]. Most commonly, the way to resolve
this is to abandon the unneeded commits (using `jj abandon <commit ID>`). If you
would like to keep both commits with this change ID, you can `jj duplicate` one
of them before abandoning it.

Usually, the different commits associated with the divergent change ID should all
appear in the log, but due to #2476, they may not. If that happens, you can
either use `jj log -r 'all()' | grep <change id>` or disable the
`revsets.short-prefixes` config option.

### How do I deal with conflicted branches ('??' after branch name)?

A [conflicted branch][branches_conflicts] is a branch that refers to multiple
different commits because jj couldn't fully resolve its desired position.
Resolving conflicted branches is usually done by setting the branch to the
correct commit using `jj branch set <commit ID>`.

Usually, the different commits associated with the conflicted branch should all
appear in the log, but if they don't you can use `jj branch list`to show all the
commits associated with it.

[branches_conflicts]: branches.md#conflicts

[change ID]: glossary.md#change-id
[commit ID]: glossary.md#commit-id
[config]: config.md

[gitignore]: https://git-scm.com/docs/gitignore

[glossary_divergent_change]: glossary.md#divergent-change

[revsets]: revsets.md

[templates]: templates.md

[this issue]: https://github.com/martinvonz/jj/issues/1531
