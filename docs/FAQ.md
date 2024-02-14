# Frequently asked questions

## General Technical Questions

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

### How can I get `jj log` to show me what `git log` would show me?

Use `jj log -r ..`. The `..` [operator] lists all visible commits in the repo, excluding the root (which is never interesting and is shared by all repos).

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

You can see the actual history of working copy changes using `jj obslog`. This
will show the history of the commits that were previously the "working-copy
commit", since the last time the change id of the working copy commit changed.
The obsolete changes will be marked as "hidden". They are still accessible with
any `jj` command (`jj diff`, for example), but you will need to use the commit
id to refer to hidden commits.

You can also use `jj obslog -r` on revisions that were previously the
working-copy revisions (or on any other revisions). Use `jj obslog -p` as an
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

### I accidentally changed files in the wrong commit, how do I move the recent changes into another commit?

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

### How do I integrate Jujutsu with Gerrit?

At the moment you'll need a script, which adds the required fields for Gerrit
like the `Change-Id` footer. Then `jj` can invoke it via an `$EDITOR` override
in an aliased command. Here's an [example][gerrit-integration] from an
contributor (look for the `jj signoff` alias).

After you have attached the `Change-Id:` footer to the commit series, you'll
have to manually invoke `git push` of `HEAD` on the underlying git repository
into the remote Gerrit branch `refs/for/$BRANCH`, where `$BRANCH` is the base
branch you want your changes to go to (e.g., `git push origin
HEAD:refs/for/main`). Using a [co-located][co-located] repo
will make the underlying git repo directly accessible from the working
directory.

We hope to integrate with Gerrit natively in the future.

## Other Questions

### What's does the name _Jujutsu_ mean, and where does it come from?

When Martin von Zweigbergk originally started the project, it was named
"[Jujube](https://en.wikipedia.org/wiki/Jujube)" after a type of fruit. This
name is what actually gave rise to the lovable and short name `jj` for the
command line tool. Later on, the the project was renamed to _Jujutsu_ &mdash; in
part because it meant we could keep the command name `jj`.

Jujutsu is a Japanese word that has two distinct interpretations in English,
each with their own kanji and romanized form:

- 柔術 _Jūjutsu_ &mdash; a family of Japanese martial arts, commonly spelled as
  "**jujitsu**" or "**jiu-jitsu**" in the West. Thankfully, we checked the
  spelling first.

- 呪術 _Jujutsu_ &mdash; roughly meaning "magic" or "sorcery".

However, the basic english word "jujutsu" on its own is somewhat ambiguous to
native Japanese speakers, because most English words don't use the long vowel
form "ū", making it unclear what the intended meaning might be when an English
speaker writes it.

For some time, we left the interpretation ambiguous.

However, we have officially chosen **second** interpretation as the official
name of the tool, 呪術 _Jujutsu_ as in "sorcery". Many of our users have said
that as a version control system, Jujutsu simply feels like magic to them, so we
think this is a fitting interpretation.

The name "Jujutsu" is also known by many of our users through the popular manga
and anime series _Jujutsu Kaisen_ (呪術廻戦). We didn't choose the name because
of the series, but we're happy so many people enjoy the connection.

### What's the proper way to refer to the project and tool?

If you are writing a technical document, blog, article, forum post, social media
endorsement, then the proper name of the project is Jujutsu (呪術 _Jujutsu_) and
should be referred to as such in the text. Use it like any other proper noun.
You don't _need_ to specify the kanji and romanized form, though it would be
courteous to your Japanese-native readers to do so for the reasons explained
above.

In contrast, the name `jj` only refers to the command line interface of the
tool, or generally the codebase of the tool itself. When writing about the
command line interface, please refer to it as `jj` with the proper typographic
code formatting, etc. (Occasionally though, the developers and contributors
themselves may refer to the project as `jj` in casual conversations around the
watercooler.)

[branches_conflicts]: branches.md#conflicts

[change ID]: glossary.md#change-id
[co-located]: glossary.md#change-id
[commit ID]: glossary.md#commit-id
[config]: config.md

[gerrit-integration]: https://gist.github.com/thoughtpolice/8f2fd36ae17cd11b8e7bd93a70e31ad6
[gitignore]: https://git-scm.com/docs/gitignore

[glossary_divergent_change]: glossary.md#divergent-change

[operator]: revsets.md#operators

[revsets]: revsets.md

[templates]: templates.md

[this issue]: https://github.com/martinvonz/jj/issues/1531
