# Using Jujutsu with GitHub and GitLab Projects

This guide assumes a basic understanding of either Git or Mercurial.

## Set up an SSH key

As of October 2023 it's recommended to set up an SSH key to work with GitHub
projects. See [GitHub's Tutorial][gh]. This restriction may be lifted in the
future, see [issue #469][http-auth] for more information and progress on
authenticated HTTP.

## Basic workflow

The simplest way to start with Jujutsu is to create a stack of commits first.
You will only need to create a bookmark when you need to push the stack to a
remote. There are two primary workflows: using a generated bookmark name or
naming a bookmark.

### Using a generated bookmark name

In this example we're letting Jujutsu auto-create a bookmark.

```shell
# Start a new commit off of the default bookmark.
$ jj new main
# Refactor some files, then add a description and start a new commit
$ jj commit -m 'refactor(foo): restructure foo()'
# Add a feature, then add a description and start a new commit
$ jj commit -m 'feat(bar): add support for bar'
# Let Jujutsu generate a bookmark name and push that to GitHub. Note that we
# push the working-copy commit's *parent* because the working-copy commit
# itself is empty.
$ jj git push -c @-
```

### Using a named bookmark

In this example, we create a bookmark named `bar` and then push it to the remote.

```shell
# Start a new commit off of the default bookmark.
$ jj new main
# Refactor some files, then add a description and start a new commit
$ jj commit -m 'refactor(foo): restructure foo()'
# Add a feature, then add a description and start a new commit
$ jj commit -m 'feat(bar): add support for bar'
# Create a bookmark so we can push it to GitHub. Note that we created the bookmark
# on the working-copy commit's *parent* because the working copy itself is empty.
$ jj bookmark create bar -r @- # `bar` now contains the previous two commits.
# Push the bookmark to GitHub (pushes only `bar`)
$ jj git push --allow-new
```

While it's possible to create a bookmark in advance and commit on top of it in a
Git-like manner, you will then need to move the bookmark manually when you create
a new commits. Unlike Git, Jujutsu will not do it automatically.

## Updating the repository

As of October 2023, Jujutsu has no equivalent to a `git pull` command (see
[issue #1039][sync-issue]). Until such a command is added, you need to use
`jj git fetch` followed by a `jj rebase -d $main_bookmark` to update your
changes.

[sync-issue]: https://github.com/martinvonz/jj/issues/1039

## Working in a Git co-located repository

After doing `jj git init --colocate`, Git will be in a [detached HEAD
state][detached], which is unusual, as Git mainly works with bookmarks. In a
co-located repository, every `jj` command will automatically synchronize
Jujutsu's view of the repo with Git's view. For example, `jj commit` updates the
HEAD of the Git repository, enabling an incremental migration.

```shell
$ nvim docs/tutorial.md
$ # Do some more work.
$ jj commit -m "Update tutorial"
# Create a bookmark on the working-copy commit's parent
$ jj bookmark create doc-update -r @-
$ jj git push --allow-new
```

## Working in a Jujutsu repository

In a Jujutsu repository, the workflow is simplified. If there's no need for
explicitly named bookmarks, you can just generate one for a change. As Jujutsu is
able to create a bookmark for a revision.

```shell
$ # Do your work
$ jj commit
$ # Push change "mw", letting Jujutsu automatically create a bookmark called
$ # "push-mwmpwkwknuz"
$ jj git push --change mw
```

## Addressing review comments

There are two workflows for addressing review comments, depending on your
project's preference. Many projects prefer that you address comments by adding
commits to your bookmark[^1]. Some projects (such as Jujutsu and LLVM) instead
prefer that you keep your commits clean by rewriting them and then
force-pushing[^2].

### Adding new commits

If your project prefers that you address review comments by adding commits on
top, you can do that by doing something like this:

```shell
$ # Create a new commit on top of the `your-feature` bookmark from above.
$ jj new your-feature
$ # Address the comments by updating the code. Then review the changes.
$ jj diff
$ # Give the fix a description and create a new working-copy on top.
$ jj commit -m 'address pr comments'
$ # Update the bookmark to point to the new commit.
$ jj bookmark set your-feature -r @-
$ # Push it to your remote
$ jj git push
```

Notably, the above workflow creates a new commit for you. The same can be
achieved without creating a new commit.

> **Warning**
> We strongly suggest to `jj new` after the example below, as all further edits
> still get amended to the previous commit.

```shell
$ # Create a new commit on top of the `your-feature` bookmark from above.
$ jj new your-feature
$ # Address the comments by updating the code. Then review the changes.
$ jj diff
$ # Give the fix a description.
$ jj describe -m 'address pr comments'
$ # Update the bookmark to point to the current commit.
$ jj bookmark set your-feature -r @
$ # Push it to your remote
$ jj git push
```

### Rewriting commits

If your project prefers that you keep commits clean, you can do that by doing
something like this:

```shell
$ # Create a new commit on top of the second-to-last commit in `your-feature`,
$ # as reviewers requested a fix there.
$ jj new your-feature- # NOTE: the trailing hyphen is not a typo!
$ # Address the comments by updating the code. Then review the changes.
$ jj diff
$ # Squash the changes into the parent commit
$ jj squash
$ # Push the updated bookmark to the remote. Jujutsu automatically makes it a
$ # force push
$ jj git push --bookmark your-feature
```

The hyphen after `your-feature` comes from the
[revset](https://github.com/martinvonz/jj/blob/main/docs/revsets.md) syntax.

## Working with other people's bookmarks

By default, `jj git clone` imports the default remote bookmark (which is usually
`main` or `master`), but `jj git fetch` doesn't import new remote bookmarks to
local bookmarks. This means that if you want to iterate or test another
contributor's bookmark, you'll need to do `jj new <bookmark>@<remote>` onto it.

If you want to import all remote bookmarks including inactive ones, set
`git.auto-local-bookmark = true` in the config file. Then you can specify a
contributor's bookmark as `jj new <bookmark>` instead of `jj new <bookmark>@<remote>`.

You can find more information on that setting [here][auto-bookmark].

## Using GitHub CLI

GitHub CLI will have trouble finding the proper Git repository path in jj repos
that aren't [co-located](./git-compatibility.md#co-located-jujutsugit-repos)
(see [issue #1008]). You can configure the `$GIT_DIR` environment variable to
point it to the right path:

```shell
$ GIT_DIR=.jj/repo/store/git gh issue list
```

You can make that automatic by installing [direnv](https://direnv.net) and
defining hooks in a `.envrc` file in the repository root to configure `$GIT_DIR`.
Just add this line into `.envrc`:

```shell
export GIT_DIR=$PWD/.jj/repo/store/git
```

and run `direnv allow` to approve it for direnv to run. Then GitHub CLI will
work automatically even in repos that aren't co-located so you can execute
commands like `gh issue list` normally.

[issue #1008]: https://github.com/martinvonz/jj/issues/1008

## Useful Revsets

Log all revisions across all local bookmarks that aren't on the main bookmark nor
on any remote:

```shell
$ jj log -r 'bookmarks() & ~(main | remote_bookmarks())'
```

Log all revisions that you authored, across all bookmarks that aren't on any
remote:

```shell
$ jj log -r 'mine() & bookmarks() & ~remote_bookmarks()'
```

Log all remote bookmarks that you authored or committed to:

```shell
$ jj log -r 'remote_bookmarks() & (mine() | committer(your@email.com))'
```

Log all descendants of the current working copy that aren't on any remote:

```shell
$ jj log -r '::@ & ~remote_bookmarks()'
```

## Merge conflicts

For a detailed overview, how Jujutsu handles conflicts, revisit
the [tutorial][tut].

[^1]:
    This is a GitHub-style review, as GitHub currently only is able to compare
    bookmarks.

[^2]:
    If you're wondering why we prefer clean commits in this project, see
    e.g. [this blog post][stacked]

[auto-bookmark]: config.md#automatic-local-bookmark-creation
[detached]: https://git-scm.com/docs/git-checkout#_detached_head
[gh]: https://docs.github.com/en/authentication/connecting-to-github-with-ssh/generating-a-new-ssh-key-and-adding-it-to-the-ssh-agent
[http-auth]: https://github.com/martinvonz/jj/issues/469
[tut]: tutorial.md#conflicts
[stacked]: https://jg.gg/2018/09/29/stacked-diffs-versus-pull-requests/

## Using several remotes

It is common to use several remotes when contributing to a shared repository.
For example, "upstream" can designate the remote where the changes will be
merged through a pull-request while "origin" is your private fork of the
project.

```shell
$ jj git clone --remote upstream https://github.com/upstream-org/repo
$ cd repo
$ jj git remote add origin git@github.com:your-org/your-repo-fork
```

This will automatically setup your repository to track the main
bookmark from the upstream repository, typically `main@upstream`
or `master@upstream`.

You might want to `jj git fetch` from "upstream" and to `jj git push`
to "origin". You can configure the default remotes to fetch from and
push to in your configuration file (for example,
`.jj/repo/config.toml`):

```toml
[git]
fetch = "upstream"
push = "origin"
```

The default for both `git.fetch` and `git.push` is "origin".

If you usually work on a project from several computers, you may
configure `jj` to fetch from both repositories by default, in order to
keep your own bookmarks synchronized through your `origin` repository:

```toml
[git]
fetch = ["upstream", "origin"]
push = "origin"
```
