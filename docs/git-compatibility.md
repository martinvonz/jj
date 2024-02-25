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
* **Authentication: Partial.** Only `ssh-agent`, a password-less key (
  only `~/.ssh/id_rsa`, `~/.ssh/id_ed25519` or `~/.ssh/id_ed25519_sk`), or
  a `credential.helper`.
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
* **Bare repositories: Yes.** You can use `jj git init --git-repo=<path>` to
  create a repo backed by a bare Git repo.
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
`jj git init --git-repo=<path to Git repo> <name>`. The repo will work similar
to a [Git worktree](https://git-scm.com/docs/git-worktree), meaning that the
working copies files and the record of the working-copy commit will be separate,
but the commits will be accessible in both repos. Use `jj git import` to update
the Jujutsu repo with changes made in the Git repo. Use `jj git export` to
update the Git repo with changes made in the Jujutsu repo.

## Creating a repo by cloning a Git repo

To create a Jujutsu repo from a remote Git URL, use `jj git clone <URL>
[<destination>]`. For example, `jj git clone
https://github.com/octocat/Hello-World` will clone GitHub's "Hello-World" repo
into a directory by the same name.


## Co-located Jujutsu/Git repos

A "co-located" Jujutsu repo is a hybrid Jujutsu/Git repo. These can be created
if you initialize the Jujutsu repo in an existing Git repo by running `jj git
init --colocate` or with `jj git clone --colocate`. The Git repo and the Jujutsu
repo then share the same working copy. Jujutsu will import and export from and
to the Git repo on every `jj` command automatically.

This mode is very convenient when tools (e.g. build tools) expect a Git repo to
be present.

It is allowed to mix `jj` and `git` commands in such a repo in any order.
However, it may be easier to keep track of what is going on if you mostly use
read-only `git` commands and use `jj` to make changes to the repo. One reason
for this (see below for more) is that `jj` commands will usually put the git
repo in a "detached HEAD" state, since in `jj` there is not concept of a
"currently tracked branch". Before doing mutating Git commands, you may need to
tell Git what the current branch should be with a `git switch` command.

You can undo the results of mutating `git` commands using `jj undo` and `jj op
restore`. Inside `jj op log`, changes by `git` will be represented as an "import
git refs" operation.

There are a few downsides to this mode of operation. Generally, using co-located
repos may require you to deal with more involved Jujutsu and Git concepts.

* Interleaving `jj` and `git` commands increases the chance of confusing branch
  conflicts or [conflicted (AKA divergent) change
  ids](glossary.md#divergent-change). These never lose data, but can be
  annoying.

    Such interleaving can happen unknowingly. For example, some IDEs can cause
  it because they automatically run `git fetch` in the background from time to
  time.

* In co-located repos with a very large number of branches or other refs, `jj`
  commands can get noticeably slower because of the automatic `jj git import`
  executed on each command. This can be mitigated by occasionally running `jj util
  gc` to speed up the import (that command includes packing the Git refs).

* Git tools will have trouble with revisions that contain conflicted files. While
  `jj` renders these files with conflict markers in the working copy, they are
  stored in a non-human-readable fashion inside the repo. Git tools will often
  see this non-human-readable representation.

* When a `jj` branch is conflicted, the position of the branch in the Git repo
  will disagree with one or more of the conflicted positions. The state of that
  branch in git will be labeled as though it belongs to a remote named "git",
  e.g. `branch@git`.

* Jujutsu will ignore Git's staging area. It will not understand merge conflicts
  as Git represents them, unfinished `git rebase` states, as well as other less
  common states a Git repository can be in.

* Colocated repositories are less resilient to
  [concurrency](technical/concurrency.md#syncing-with-rsync-nfs-dropbox-etc)
  issues if you share the repo using an NFS filesystem or Dropbox. In general,
  such use of Jujutsu is not currently thoroughly tested.

* There may still be bugs when interleaving mutating `jj` and `git` commands,
  usually having to do with a branch pointer ending up in the wrong place. We
  are working on the known ones, and are not aware of any major ones. Please
  report any new ones you find, or if any of the known bugs are less minor than
  they appear.

### Converting a repo into a co-located repo

A Jujutsu repo backed by a Git repo has a full Git repo inside, so it is
technically possible (though not officially supported) to convert it into a
co-located repo like so:

```bash
# Move the Git repo
mv .jj/repo/store/git .git
# Tell jj where to find it
echo -n '../../../.git' > .jj/repo/store/git_target
# Ignore the .jj directory in Git
echo '/*' > .jj/.gitignore
# Make the Git repository non-bare and set HEAD
git config --unset core.bare
jj st
```

We may officially support this in the future. If you try this, we would
appreciate feedback and bug reports.

## Branches

TODO: Describe how branches are mapped


## Format mapping details

Paths are assumed to be UTF-8. I have no current plans to support paths with
other encodings.

Commits created by `jj` have a ref starting with `refs/jj/` to prevent GC.

Commit metadata that cannot be represented in Git commits (such as the Change
ID and information about conflicts) is stored outside of the Git repo (currently
in `.jj/store/extra/`).

Commits with conflicts cannot be represented in Git. They appear in the Git
commit as as root directories called`.jjconflict-base-*/` and
`.jjconflict-side-*/`. Note that the purpose of this representation is only to
prevent GC of the relevant trees; the authoritative information is in the
Git-external storage mentioned in the paragraph above. As long as you use `jj`
commands to work with them, you won't notice those paths. If, on the other hand,
you use e.g. `git switch` to check one of them out, you will see those
directories in your working copy. If you then run e.g. `jj status`, the
resulting snapshot will contain those directories, making it look like they
replaced all the other paths in your repo. You will probably want to run
`jj abandon` to get back to the state with the unresolved conflicts.
