# Git compatibility

Jujutsu has two backends for storing commits. One of them uses a regular Git
repo, which means that you can collaborate with Git users without them even
knowing that you're not using the `git` CLI.

See `jj help git` for help about the `jj git` family of commands, and e.g.
`jj help git push` for help about a specific command (use `jj git push -h` for
briefer help).


## Supported features

The following list describes which Git features Jujutsu is compatible with. For
a comparison with Git, including how workflows are different, see the
[Git-comparison doc](git-comparison.md).

* **Configuration: Partial.** The only configuration from Git (e.g. in
  `~/.gitconfig`) that's respected is the following. Feel free to file a bug if
  you miss any particular configuration options.
  * The configuration of remotes (`[remote "<name>"]`).
  * `core.excludesFile`
* **Authentication: Partial.** Only `ssh-agent`, a password-less key file at
  `~/.ssh/id_rsa` (and only at exactly that path), or a `credential.helper`.
* **Branches: Yes.** You can read more about
  [how branches work in Jujutsu](branches.md)
  and [how they interoperate with Git](#branches).
* **Tags: Partial.** You can check out tagged commits by name (pointed to be
  either annotated or lightweight tags), but you cannot create new tags.
* **.gitignore: Yes.** Ignores in `.gitignore` files are supported. So are
  ignores in `.git/info/exclude` or configured via Git's `core.excludesfile`
  config. The `.gitignore` support uses a native implementation, so please
  report a bug if you notice any difference compared to `git`.  
* **.gitattributes: No.** There's [#53](https://github.com/martinvonz/jj/issues/53)
  about adding support for at least the `eol` attribute.
* **Hooks: No.** There's [#405](https://github.com/martinvonz/jj/issues/405)
  specifically for providing the checks from https://pre-commit.com.
* **Merge commits: Yes.** Octopus merges (i.e. with more than 2 parents) are
  also supported.
* **Detached HEAD: Yes.** Jujutsu supports anonymous branches, so this is a
  natural state.
* **Orphan branch: Yes.** Jujutsu has a virtual root commit that appears as
  parent of all commits Git would call "root commits".
* **Staging area: Kind of.** The staging area will be ignored. For example,
  `jj diff` will show a diff from the Git HEAD to the working copy. There are
  [ways of fulfilling your use cases without a staging
  area](https://github.com/martinvonz/jj/blob/main/docs/git-comparison.md#the-index).  
* **Garbage collection: Yes.** It should be safe to run `git gc` in the Git
  repo, but it's not tested, so it's probably a good idea to make a backup of
  the whole workspace first. There's [no garbage collection and repacking of
  Jujutsu's own data structures yet](https://github.com/martinvonz/jj/issues/12),
  however.
* **Bare repositories: Yes.** You can use `jj init --git-repo=<path>` to create
  a repo backed by a bare Git repo.
* **Submodules: No.** They will not show up in the working copy, but they will
  not be lost either.
* **Partial clones: No.** We use the [libgit2](https://libgit2.org/) library,
  which [doesn't have support for partial clones](https://github.com/libgit2/libgit2/issues/5564).
* **Shallow clones: No.** We use the [libgit2](https://libgit2.org/) library,
  which [doesn't have support for shallow clones](https://github.com/libgit2/libgit2/issues/3058).
* **git-worktree: No.** However, there's native support for multiple working
  copies backed by a single repo. See the `jj workspace` family of commands.
* **Sparse checkouts: No.** However, there's native support for sparse
  checkouts. See the `jj sparse` command.
* **Signed commits: No.** ([#58](https://github.com/martinvonz/jj/issues/58))
* **Git LFS: No.** ([#80](https://github.com/martinvonz/jj/issues/80))


## Creating an empty repo

To create an empty repo using the Git backend, use `jj init --git <name>`. Since
the command creates a Jujutsu repo, it will have a `.jj/` directory. The
underlying Git repo will be inside of that directory (currently in
`.jj/repo/store/git/`).


## Creating a repo backed by an existing Git repo

To create a Jujutsu repo backed by a Git repo you already have on disk, use
`jj init --git-repo=<path to Git repo> <name>`. The repo will work similar to a
[Git worktree](https://git-scm.com/docs/git-worktree), meaning that the working
copies files and the record of the working-copy commit will be separate, but the
commits will be accessible in both repos. Use `jj git import` to update the
Jujutsu repo with changes made in the Git repo. Use `jj git export` to update
the Git repo with changes made in the Jujutsu repo.

### Co-located Jujutsu/Git repos

If you initialize the Jujutsu repo in the same working copy as the Git repo by
running `jj init --git-repo=.`, then the import and export will happen
automatically on every command (because not doing that makes it very confusing
when the working copy has changed in Git but not in Jujutsu or vice versa). We
call such repos "co-located".

This mode is meant to make it easier to start using readonly `jj` commands in an
existing Git repo. You should then be able to switch to using mutating `jj`
commands and readonly Git commands. It's also useful when tools (e.g. build
tools) expect a Git repo to be present.

The mode is new and not tested much, and interleaving mutating `jj` and `git`
commands might not work well (feel free to report bugs).


## Creating a repo by cloning a Git repo

To create a Jujutsu repo from a remote Git URL, use `jj git clone <URL>
[<destination>]`. For example, `jj git clone
https://github.com/octocat/Hello-World` will clone GitHub's "Hello-World" repo
into a directory by the same name.


## Branches

TODO: Describe how branches are mapped


## Format mapping details

Paths are assumed to be UTF-8. I have no current plans to support paths with
other encodings.

Commits created by `jj` have a ref starting with `refs/jj/` to prevent GC.

Commit metadata that cannot be represented in Git commits (such as the Change
ID) is stored outside of the Git repo (currently in `.jj/store/extra/`).

Paths with conflicts cannot be represented in Git. They appear as files with
a `.jjconflict` suffix in the Git repo. They contain a JSON representation with
information about the conflict. They are not meant to be human-readable.
