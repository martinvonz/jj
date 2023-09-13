# Using Jujutsu with GitHub and GitLab Projects

This guide assumes a basic understanding of either Git or Mercurial.

## Set up an SSH key

As of December 2022 it's recommended to set up an SSH key to work with GitHub
projects. See [GitHub's Tutorial][gh]. This restriction may be lifted in the
future, see [issue #469][http-auth] for more information and progress on
authenticated http.

## Basic workflow

The simplest way to start with Jujutsu, is creating a stack of commits, before
creating any branch.

```shell
# Start a new commit off of `main`
$ jj new main
# Refactor some files, then add a description and start a new commit
$ jj commit -m 'refactor(foo): restructure foo()'
# Add a feature, then add a description and start a new commit
$ jj commit -m 'feat(bar): add support for bar'
# Create a branch so we can push it to GitHub
$ jj branch create bar -r @-
# Push the branch to GitHub (pushes only `bar`)
$ jj git push
```

While it's possible to create a branch and commit on top of it in a Git like
manner, it's not recommended, as no further commits will be placed on the
branch.

## Updating the repository.

As of December 2022, Jujutsu has no equivalent to a `git pull` command. Until
such a command is added, you need to use `jj git fetch` followed by a
`jj rebase -d $main_branch` to update your changes.

## Working in a Git co-located repository

After doing `jj init --git-repo=.`, git will be in
a [detached HEAD state][detached], which is unusual, as git mainly works with
branches. In a co-located repository, `jj` isn't the source of truth. But
Jujutsu allows an incremental migration, as `jj commit` updates the HEAD of the
git repository.

```shell
$ nvim docs/tutorial.md
$ # Do some more work.
$ jj commit -m "Update tutorial"
$ jj branch create doc-update
$ # Move the previous revision to doc-update.
$ jj branch set doc-update -r @-
$ jj git push
```

## Working in a Jujutsu repository

In a Jujutsu repository, the workflow is simplified. If there's no need for
explicitly named branches, you just can generate one for a change. As Jujutsu is
able to create a branch for a revision.

```shell
$ # Do your work
$ jj commit
$ # Jujutsu automatically creates a branch
$ jj git push --change $revision
```

## Addressing review comments

There are two workflows for addressing review comments, depending on your
project's preference. Many projects prefer that you address comments by adding
commits to your branch[^1]. Some projects (such as Jujutsu and LLVM) instead
prefer that you keep your commits clean by rewriting them and then
force-pushing[^2].

### Adding new commits

If your project prefers that you address review comments by adding commits on
top, you can do that by doing something like this:

```shell
$ # Create a new commit on top of the `your-feature` branch from above.
$ jj new your-feature
$ # Address the comments, by updating the code
$ jj diff
$ # Give the fix a description and create a new working-copy on top.
$ jj commit -m 'address pr comments'
$ # Update the branch to point to the new commit.
$ jj branch set your-feature -r @-
$ # Push it to your remote
$ jj git push.
```

### Rewriting commits

If your project prefers that you keep commits clean, you can do that by doing
something like this:

```shell
$ # Create a new commit on top of the second-to-last commit in `your-feature`,
$ # as reviews requested a fix there.
$ jj new your-feature
$ # Address the comments by updating the code
$ # Review the changes
$ jj diff
$ # Squash the changes into the parent commit
$ jj squash
$ # Push the updated branch to the remote. Jujutsu automatically makes it a force push
$ jj git push --branch your-feature
```

## Using GitHub CLI

GitHub CLI will have trouble finding the proper git repository path in jj repos
that aren't [co-located](./git-compatibility.md#co-located-jujutsugit-repos)
(see [issue #1008]). You can configure the `$GIT_DIR` environment variable to
point it to the right path:

```shell
$ GIT_DIR=.jj/repo/store/git gh issue list
```

You can make that automatic by installing [direnv](https://direnv.net) and
defining hooks in a .envrc file in the repository root to configure `$GIT_DIR`.
Just add this line into .envrc:

```shell
export GIT_DIR=$PWD/.jj/repo/store/git
```

and run `direnv allow` to approve it for direnv to run. Then GitHub CLI will
work automatically even in repos that aren't co-located so you can execute
commands like `gh issue list` normally.

[issue #1008]: https://github.com/martinvonz/jj/issues/1008

## Useful Revsets

Log all revisions across all local branches, which aren't on the main branch nor
on any remote
`jj log -r 'branches() & ~(main | remote_branches())'`
Log all revisions which you authored, across all branches which aren't on any
remote
`jj log -r 'mine() & branches() & ~remote_branches()'`
Log all remote branches, which you authored or committed to
`jj log -r 'remote_branches() & (mine() | committer(your@email.com))'`
Log all descendants of the current working copy, which aren't on a remote
`jj log -r '::@ & ~remote_branches()'`

## Merge conflicts

For a detailed overview, how Jujutsu handles conflicts, revisit
the [tutorial][tut].

[^1]: This is a GitHub Style review, as GitHub currently only is able to compare
branches.

[^2]: If you're wondering why we prefer clean commits in this project, see
e.g. [this blog post][stacked]

[detached]: https://git-scm.com/docs/git-checkout#_detached_head

[gh]: https://docs.github.com/en/authentication/connecting-to-github-with-ssh/generating-a-new-ssh-key-and-adding-it-to-the-ssh-agent

[http-auth]: https://github.com/martinvonz/jj/issues/469

[tut]: tutorial.md#Conflicts

[stacked]: https://jg.gg/2018/09/29/stacked-diffs-versus-pull-requests/

## Using several remotes

It is common to use several remotes when contributing to a shared repository.
For example,
"upstream" can designate the remote where the changes will be merged through a
pull-request while "origin" is your private fork of the project. In this case,
you might want to
`jj git fetch` from "upstream" and to `jj git push` to "origin".

You can configure the default remotes to fetch from and push to in your
configuration file
(for example `.jj/repo/config.toml`):

```toml
[git]
fetch = "upstream"
push = "origin"
```

The default for both `git.fetch` and `git.push` is "origin".
