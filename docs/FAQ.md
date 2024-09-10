# Frequently asked questions

### Why does my bookmark not move to the new commit after `jj new/commit`?

If you're familiar with Git, you might expect the current bookmark to move forward
when you commit. However, Jujutsu does not have a concept of a "current bookmark".

To move bookmarks, use `jj bookmark set`.

### I made a commit and `jj git push --all` says "Nothing changed" instead of pushing it. What do I do?

`jj git push --all` pushes all _bookmarks_, not all revisions. You have two
options:

* Using `jj git push --change` will automatically create a bookmark and push it.
* Using `jj bookmark` commands to create or move a bookmark to either the commit
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

### How can I get `jj log` to show me what `git log` would show me?

Use `jj log -r ..`. The `..` [operator] lists all visible commits in the repo, excluding the root (which is never interesting and is shared by all repos).

### Should I co-locate my repository?

Co-locating a Jujutsu repository allows you to use both Jujutsu and Git in the
same working copy. The benefits of doing so are:

- You can use Git commands when you're not sure how to do something with
  Jujutsu, Jujutsu hasn't yet implemented a feature (e.g., bisection), or you
  simply prefer Git in some situations.

- Tooling that expects a Git repository still works (IDEs, build tooling, etc.)

The [co-location documentation describes the
drawbacks](git-compatibility.md#co-located-jujutsugit-repos) but the most
important ones are:

- Interleaving `git` and `jj` commands may create confusing branch conflicts or
  divergent changes.

- If the working copy commit or its parent contain any conflicted files, tools
  expecting a Git repo may interpret the commit contents or its diff in a wrong
  and confusing way. You should avoid doing mutating operations with Git tools
  and ignore the confusing information such tools present for conflicted commits
  (unless you are curious about [the details of how `jj` stores
  conflicts](technical/conflicts.md)). See
  [\#3979](https://github.com/martinvonz/jj/issues/3979) for plans to improve
  this situation.

- Jujutsu commands may be a little slower in very large repositories due to
  importing and exporting changes to Git. Most repositories are not noticeably
  affected by this. 

If you primarily use Jujutsu to modify the repository, the drawbacks are
unlikely to affect you. Try co-locating while you learn Jujutsu, then switch if
you find a specific reason not to co-locate.

### `jj` is said to record the working copy after `jj log` and every other command. Where can I see these automatic "saves"?  

Indeed, every `jj` command updates the current "working-copy" revision, marked 
with `@` in `jj log`. You can notice this by how the [commit ID] of the
working copy revision changes when it's updated. Note that, unless you move to
another revision (with `jj new` or `jj edit`, for example), the [change ID] will 
not change.

If you expected to see a historical view of your working copy changes in the
parent-child relationships between commits you can see in `jj log`, this is
simply not what they mean. What you can see in `jj log` is that after the
working copy commit gets amended (after any edit), the commit ID changes.

You can see the actual history of working copy changes using `jj evolog`. This
will show the history of the commits that were previously the "working-copy
commit", since the last time the change id of the working copy commit changed.
The obsolete changes will be marked as "hidden". They are still accessible with
any `jj` command (`jj diff`, for example), but you will need to use the commit
id to refer to hidden commits.

You can also use `jj evolog -r` on revisions that were previously the
working-copy revisions (or on any other revisions). Use `jj evolog -p` as an
easy way to see the evolution of the commit's contents.

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

If you have changes you _never_ want to put in a public commit, see: [How can I
keep my scratch files in the repository without committing
them?](#how-can-i-keep-my-scratch-files-in-the-repository-without-committing-them)

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

### How can I keep my scratch files in the repository without committing them?

You can set `snapshot.auto-track` to only start tracking new files matching the
configured pattern (e.g. `"none()"`). Changes to already tracked files will
still be snapshotted by every command.

You can keep your notes and other scratch files in the repository, if you add
a wildcard pattern to either the repo's `gitignore` or your global `gitignore`.
Something like `*.scratch` or `*.scratchpad` should do, after that rename the
files you want to keep around to match the pattern.

If you keep your scratch files in their own directory with no tracked files, you
can create a `.gitignore` file in that directory containing only `*`. This will
ignore everything in the directory including the `.gitignore` file itself.

If `$EDITOR` integration is important, something like `scratchpad.*` may be more
helpful, as you can keep the filename extension intact (it
matches `scratchpad.md`, `scratchpad.rs` and more). Another option is to add a 
directory to the global `.gitignore` which then stores all your temporary files
and notes. For example, you could add `scratch/` to `~/.git/ignore` and then 
store arbitrary files in `<your-git-repo>/scratch/`.

You can find more details on `gitignore` files [here][gitignore].

### How can I avoid committing my local-only changes to tracked files?

Suppose your repository tracks a file like `secret_config.json`, and you make
some changes to that file to work locally. Since Jujutsu automatically commits
the working copy, there's no way to prevent Jujutsu from committing changes to
the file. But, you never want to push those changes to the remote repository. 

One solution is to keep these changes in a separate commit branched from the
trunk. To use those changes in your working copy, _merge_ the private commit
into your branch.

Suppose you have a commit "Add new feature":

```shell
$ jj log
@  xxxxxxxx me@example.com 2024-08-21 11:13:21 ef612875
│  Add new feature
◉  yyyyyyyy me@example.com 2024-08-21 11:13:09 main b624cf12
│  Existing work
~
```

First, create a new commit branched from main and add your private changes:

```shell
$ jj new main -m "private: my credentials"
Working copy now at: wwwwwwww 861de9eb (empty) private: my credentials
Parent commit      : yyyyyyyy b624cf12 main | Existing work
Added 0 files, modified 1 files, removed 0 files

$ echo '{ "password": "p@ssw0rd1" }' > secret_config.json
```

Now create a merge commit with the branch you're working on and the private
commit:

```shell
$ jj new xxxxxxxx wwwwwwww
Working copy now at: vvvvvvvv ac4d9fbe (empty) (no description set)
Parent commit      : xxxxxxxx ef612875 Add new feature
Parent commit      : wwwwwwww 2106921e private: my credentials
Added 0 files, modified 1 files, removed 0 files

$ jj log
@    vvvvvvvv me@example.com 2024-08-22 08:57:40 ac4d9fbe
├─╮  (empty) (no description set)
│ ◉  wwwwwwww me@example.com 2024-08-22 08:57:40 2106921e
│ │  private: my credentials
◉ │  xxxxxxxx me@example.com 2024-08-21 11:13:21 ef612875
├─╯  Add new feature
◉  yyyyyyyy me@example.com 2024-08-21 11:13:09 main b624cf12
│  Existing work
~
```

Now you're ready to work:

- Your work in progress _xxxxxxxx_ is the first parent of the merge commit.
- The private commit _wwwwwwww_ is the second parent of the merge commit.
- The working copy (_vvvvvvvv_) contains changes from both.

As you work, squash your changes using `jj squash --into xxxxxxxx`. Or, you can
keep your changes in a separate commit and remove _ttsqqnrx_ as a parent:

```shell
# Remove the private commit as a parent
$ jj rebase -r vvvvvvvv -d xxxxxxxx

# Create a new merge commit to work in
$ jj new vvvvvvvv wwwwwwww
```

To avoid pushing change _wwwwwwww_ by mistake, use the configuration
[git.private-commits](config.md#set-of-private-commits):

```
$ jj config set --user git.private-commits 'description(glob:"private:*")'
```

### I accidentally changed files in the wrong commit, how do I move the recent changes into another commit?

Use `jj evolog -p` to see how your working-copy commit has evolved. Find the
commit you want to restore the contents to. Let's say the current commit (with
the changes intended for a new commit) are in commit X and the state you wanted
is in commit Y. Note the commit id (normally in blue at the end of the line in
the log output) of each of them. Now use `jj new` to create a new working-copy
commit, then run `jj restore --from Y --to @-` to restore the parent commit
to the old state, and `jj restore --from X` to restore the new working-copy
commit to the new state.

### How do I resume working on an existing change?

There are two ways to resume working on an earlier change: `jj new` then `jj squash`,
and `jj edit`. The first is generally recommended, but `jj edit` can be useful. When 
you use `jj edit`, the revision is directly amended with your new changes, making it
difficult to tell what exactly you change. You should avoid using `jj edit` when the
revision has a conflict, as you may accidentally break the plain-text annotations on
your state without realising.

To start, use `jj new <rev>` to create a change based on that earlier revision. Make
your edits, then use `jj squash` to update the earlier revision with those edits.
For when you would use git stashing, use `jj edit <rev>` for expected behaviour. 
Other workflows may prefer `jj edit` as well.

### How do I deal with divergent changes ('??' after the [change ID])?

A [divergent change][glossary_divergent_change] represents a change that has two
or more visible commits associated with it. To refer to such commits, you must
use their [commit ID]. Most commonly, the way to resolve
this is to abandon the unneeded commits (using `jj abandon <commit ID>`). If you
would like to keep both commits with this change ID, you can `jj duplicate` one
of them before abandoning it.

### How do I deal with conflicted bookmarks ('??' after bookmark name)?

A [conflicted bookmark][bookmarks_conflicts] is a bookmark that refers to multiple
different commits because jj couldn't fully resolve its desired position.
Resolving conflicted bookmarks is usually done by setting the bookmark to the
correct commit using `jj bookmark set <commit ID>`.

Usually, the different commits associated with the conflicted bookmark should all
appear in the log, but if they don't you can use `jj bookmark list`to show all the
commits associated with it.

### How do I integrate Jujutsu with Gerrit?

At the moment you'll need a script, which adds the required fields for Gerrit
like the `Change-Id` footer. Then `jj` can invoke it via an `$EDITOR` override
in an aliased command. Here's an [example][gerrit-integration] from an
contributor (look for the `jj signoff` alias).

After you have attached the `Change-Id:` footer to the commit series, you'll
have to manually invoke `git push` of `HEAD` on the underlying git repository
into the remote Gerrit bookmark `refs/for/$BRANCH`, where `$BRANCH` is the base
bookmark you want your changes to go to (e.g., `git push origin
HEAD:refs/for/main`). Using a [co-located][co-located] repo
will make the underlying git repo directly accessible from the working
directory.

We hope to integrate with Gerrit natively in the future.

[bookmarks_conflicts]: bookmarks.md#conflicts

[change ID]: glossary.md#change-id
[co-located]: glossary.md#co-located-repos
[commit ID]: glossary.md#commit-id
[config]: config.md

[gerrit-integration]: https://gist.github.com/thoughtpolice/8f2fd36ae17cd11b8e7bd93a70e31ad6
[gitignore]: https://git-scm.com/docs/gitignore

[glossary_divergent_change]: glossary.md#divergent-change

[operator]: revsets.md#operators

[revsets]: revsets.md

[templates]: templates.md

[this issue]: https://github.com/martinvonz/jj/issues/1531
