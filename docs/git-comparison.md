# Comparison with Git

## Introduction

This document attempts to describe how Jujutsu is different from Git. See
[the Git-compatibility doc](git-compatibility.md) for information about how
the `jj` command interoperates with Git repos.


## Overview

Here is a list of conceptual differences between Jujutsu and Git, along with
links to more details where applicable and available. There's a
[table further down](#command-equivalence-table) explaining how to achieve
various use cases.

* **The working copy is automatically committed.** That results in a simpler and
  more consistent CLI because the working copy is now treated like any other
  commit. [Details](working-copy.md).
* **There's no index (staging area).** Because the working copy is automatically
  committed, an index-like concept doesn't make sense. The index is very similar
  to an intermediate commit between `HEAD` and the working copy, so workflows
  that depend on it can be modeled using proper commits instead. Jujutsu has
  excellent support for moving changes between commits. [Details](#the-index).
* **No need for branch names (but they are supported).** Git lets you check out
  a commit without attaching a branch. It calls this state "detached HEAD". This
  is the normal state in Jujutsu (there's actually no way -- yet, at least -- to
  have an active branch). However, Jujutsu keeps track of all visible heads
  (leaves) of the commit graph, so the commits won't get lost or
  garbage-collected.
* **No current branch.** Git lets you check out a branch, making it the 'current
  branch', and new commits will automatically update the branch. This is
  necessary in Git because Git might otherwise lose track of the new commits.
  Jujutsu does not have a 'current branch'; instead, you update branches
  manually. For example, if you check out a commit with a branch, new commits
  are created on top of the branch, then you issue a later command to update the
  branch.
* **Conflicts can be committed.** No commands fail because of merge conflicts.
  The conflicts are instead recorded in commits and you can resolve them later.
  [Details](conflicts.md).
* **Descendant commits are automatically rebased.** Whenever you rewrite a
  commit (e.g. by running `jj rebase`), all its descendants commits will
  automatically be rebased on top. Branches pointing to it will also get
  updated, and so will the working copy if it points to any of the rebased
  commits.
* **Branches are identified by their names (across remotes).** For example, if
  you pull from a remote that has a `main` branch, you'll get a branch by that
  name in your local repo as well. If you then move it and push back to the
  remote, the `main` branch on the remote will be updated.
 [Details](branches.md).
* **The operation log replaces reflogs.** The operation log is similar to
  reflogs, but is much more powerful. It keeps track of atomic updates to all
  refs at once (Jujutsu thus improves on Git's per-ref history much in the same
  way that Subversion improved on RCS's per-file history). The operation log
  powers e.g. the undo functionality. [Details](operation-log.md)
* **There's a single, virtual root commit.** Like Mercurial, Jujutsu has a
  virtual commit (with a hash consisting of only zeros) called the "root commit"
  (called the "null revision" in Mercurial). This commit is a common ancestor of
  all commits. That removes the awkward state Git calls the "unborn branch"
  state (which is the state a newly initialized Git repo is in), and related
  command-line flags (e.g. `git rebase --root`, `git checkout --orphan`).


## The index

Git's ["index"](https://git-scm.com/book/en/v2/Git-Tools-Reset-Demystified) has
multiple roles. One role is as a cache of file system information. Jujutsu has
something similar. Unfortunately, Git exposes the index to the user, which makes
the CLI unnecessarily complicated (learning what the different flavors of
`git reset` do, especially when combined with commits and/or paths, usually
takes a while). Jujutsu, like Mercurial, doesn't make that mistake.

As a Git power-user, you may think that you need the power of the index to
commit only part of the working copy. However, Jujutsu provides commands for
more directly achieving most use cases you're used to using Git's index for. For
example, to create a commit from part of the changes in the working copy, you
might be used to using `git add -p; git commit`. With Jujutsu, you'd instead
use `jj split` to split the working-copy commit into two commits. To add more
changes into the parent commit, which you might normally use
`git add -p; git commit --amend` for, you can instead use `jj squash -i` to
choose which changes to move into the parent commit, or `jj squash <file>` to
move a specific file.


## Command equivalence table

Note that all `jj` commands can be run on any commit (not just the working-copy
commit), but that's left out of the table to keep it simple. For example,
`jj squash/amend -r <revision>` will move the diff from that revision into its
parent.

<table>
  <thead>
    <tr>
      <th>Use case</th>
      <th>Jujutsu command</th>
      <th>Git command</th>
    </tr>
  </thead>
  <tbody>
    <tr>
      <td>Create a new repo</td>
      <td><code>jj git init [--colocate]</code></td>
      <td><code>git init</code></td>
    </tr>
    <tr>
      <td>Clone an existing repo</td>
      <td><code>jj git clone &lt;source&gt; &lt;destination&gt;</code> (there is no support
          for cloning non-Git repos yet)</td>
      <td><code>git clone &lt;source&gt; &lt;destination&gt;</code></td>
    </tr>
    <tr>
      <td>Update the local repo with all branches from a remote</td>
      <td><code>jj git fetch [--remote &lt;remote&gt;]</code> (there is no
          support for fetching into non-Git repos yet)</td>
      <td><code>git fetch [&lt;remote&gt;]</code></td>
    </tr>
    <tr>
      <td>Update a remote repo with all branches from the local repo</td>
      <td><code>jj git push --all [--remote &lt;remote&gt;]</code> (there is no
          support for pushing from non-Git repos yet)</td>
      <td><code>git push --all [&lt;remote&gt;]</code></td>
    </tr>
    <tr>
      <td>Update a remote repo with a single branch from the local repo</td>
      <td><code>jj git push --branch &lt;branch name&gt;
                [--remote &lt;remote&gt;]</code> (there is no support for
                pushing from non-Git repos yet)</td>
      <td><code>git push &lt;remote&gt; &lt;branch name&gt;</code></td>
    </tr>
    <tr>
      <td>Show summary of current work and repo status</td>
      <td><code>jj st</code></td>
      <td><code>git status</code></td>
    </tr>
    <tr>
      <td>Show diff of the current change</td>
      <td><code>jj diff</code></td>
      <td><code>git diff HEAD</code></td>
    </tr>
    <tr>
      <td>Show diff of another change</td>
      <td><code>jj diff -r &lt;revision&gt;</code></td>
      <td><code>git diff &lt;revision&gt;^ &lt;revision&gt;</code></td>
    </tr>
    <tr>
      <td>Show diff from another change to the current change</td>
      <td><code>jj diff --from &lt;revision&gt;</code></td>
      <td><code>git diff &lt;revision&gt;</code></td>
    </tr>
    <tr>
      <td>Show diff from change A to change B</td>
      <td><code>jj diff --from A --to B</code></td>
      <td><code>git diff A B</code></td>
    </tr>
    <tr>
    <tr>
      <td>Show description and diff of a change</td>
      <td><code>jj show &lt;revision&gt;</code></td>
      <td><code>git show &lt;revision&gt;</code></td>
    </tr>
    <tr>
      <td>Add a file to the current change</td>
      <td><code>touch filename</code></td>
      <td><code>touch filename; git add filename</code></td>
    </tr>
    <tr>
      <td>Remove a file from the current change</td>
      <td><code>rm filename</code></td>
      <td><code>git rm filename</code></td>
    </tr>
    <tr>
      <td>Modify a file in the current change</td>
      <td><code>echo stuff >> filename</code></td>
      <td><code>echo stuff >> filename</code></td>
    </tr>
    <tr>
      <td>Finish work on the current change and start a new change</td>
      <td><code>jj commit</code></td>
      <td><code>git commit -a</code></td>
    </tr>
    <tr>
      <td>See log of ancestors of the current commit</td>
      <td><code>jj log -r ::@</code></td>
      <td><code>git log --oneline --graph --decorate</code></td>
    </tr>
    <tr>
      <td>See log of all reachable commits</td>
      <td><code>jj log -r 'all()'</code> or <code>jj log -r ::</code></td>
      <td><code>git log --oneline --graph --decorate --branches</code></td>
    </tr>
    <tr>
      <td>Show log of commits not on the main branch</td>
      <td><code>jj log</code></td>
      <td>(TODO)</td>
    </tr>
    <tr>
      <td>Search among files versioned in the repository</td>
      <td><code>grep foo $(jj files)</code>, or <code>rg --no-require-git foo</code></td>
      <td><code>git grep foo</code></td>
    </tr>
    <tr>
      <td>Abandon the current change and start a new change</td>
      <td><code>jj abandon</code></td>
      <td><code>git reset --hard</code> (cannot be undone)</td>
    </tr>
    <tr>
      <td>Make the current change empty</td>
      <td><code>jj restore</code></td>
      <td><code>git reset --hard</code> (same as abandoning a change since Git
          has no concept of a "change")</td>
    </tr>
    <tr>
      <td>Abandon the parent of the working copy, but keep its diff in the working copy</td>
      <td><code>jj squash --from @-</code></td>
      <td><code>git reset --soft HEAD~</code></td>
    </tr>
    <tr>
      <td>Discard working copy changes in some files</td>
      <td><code>jj restore &lt;paths&gt;...</code></td>
      <td><code>git restore &lt;paths&gt;...</code> or <code>git checkout HEAD -- &lt;paths&gt;...</code></td>
    </tr>
    <tr>
      <td>Edit description (commit message) of the current change</td>
      <td><code>jj describe</code></td>
      <td>Not supported</td>
    </tr>
    <tr>
      <td>Edit description (commit message) of the previous change</td>
      <td><code>jj describe @-</code></td>
      <td><code>git commit --amend</code> (first make sure that nothing is
          staged)</td>
    </tr>
    <tr>
      <td>Temporarily put away the current change</td>
      <td><code>jj new @-</code> (the old working-copy commit remains as a sibling commit)<br />
          (the old working-copy commit X can be restored with <code>jj edit X</code>)</td>
      <td><code>git stash</code></td>
    </tr>
    <tr>
      <td>Start working on a new change based on the &lt;main&gt; branch</td>
      <td><code>jj new main</code></td>
      <td><code>git switch -c topic main</code> or
        <code>git checkout -b topic main</code> (may need to stash or commit
        first)</td>
    </tr>
    <tr>
      <td>Move branch A onto branch B</td>
      <td><code>jj rebase -b A -d B</code></td>
      <td><code>git rebase B A</code>
          (may need to rebase other descendant branches separately)</td>
    </tr>
    <tr>
      <td>Move change A and its descendants onto change B</td>
      <td><code>jj rebase -s A -d B</code></td>
      <td><code>git rebase --onto B A^ &lt;some descendant branch&gt;</code>
          (may need to rebase other descendant branches separately)</td>
    </tr>
    <tr>
      <td>Reorder changes from A-B-C-D to A-C-B-D</td>
      <td><code>jj rebase -r C -d A; jj rebase -s B -d C</code> (pass change IDs,
          not commit IDs, to not have to look up commit ID of rewritten C)</td>
      <td><code>git rebase -i A</code></td>
    </tr>
    <tr>
      <td>Move the diff in the current change into the parent change</td>
      <td><code>jj squash/amend</code></td>
      <td><code>git commit --amend -a</code></td>
    </tr>
    <tr>
      <td>Interactively move part of the diff in the current change into the
          parent change</td>
      <td><code>jj squash/amend -i</code></td>
      <td><code>git add -p; git commit --amend</code></td>
    </tr>
    <tr>
      <td>Move the diff in the working copy into an ancestor</td>
      <td><code>jj squash --into X</code></td>
      <td><code>git commit --fixup=X; git rebase -i --autosquash X^</code></td>
    </tr>
    <tr>
      <td>Interactively move part of the diff in an arbitrary change to another
          arbitrary change</td>
      <td><code>jj squash -i --from X --into Y</code></td>
      <td>Not supported</td>
    </tr>
    <tr>
      <td>Interactively split the changes in the working copy in two</td>
      <td><code>jj split</code></td>
      <td><code>git commit -p</code></td>
    </tr>
    <tr>
      <td>Interactively split an arbitrary change in two</td>
      <td><code>jj split -r &lt;revision&gt;</code></td>
      <td>Not supported (can be emulated with the "edit" action in
          <code>git rebase -i</code>)</td>
    </tr>
    <tr>
      <td>Interactively edit the diff in a given change</td>
      <td><code>jj diffedit -r &lt;revision&gt;</code></td>
      <td>Not supported (can be emulated with the "edit" action in
          <code>git rebase -i</code>)</td>
    </tr>
    <tr>
      <td>Resolve conflicts and continue interrupted operation</td>
      <td><code>echo resolved > filename; jj squash/amend</code> (operations
          don't get interrupted, so no need to continue)</td>
      <td><code>echo resolved > filename; git add filename; git
          rebase/merge/cherry-pick --continue</code></td>
    </tr>
    <tr>
      <td>Create a copy of a commit on top of another commit</td>
      <td><code>jj duplicate &lt;source&gt;; jj rebase -r &lt;duplicate commit&gt; -d &lt;destination&gt;</code>
          (there's no single command for it yet)</td>
      <td><code>git co &lt;destination&gt;; git cherry-pick &lt;source&gt;</code></td>
    </tr>
    <tr>
      <td>Find the root of the working copy (or check if in a repo)</td>
      <td><code>jj workspace root</code></td>
      <td><code>git rev-parse --show-toplevel</code></td>
    </tr>
    <tr>
      <td>List branches</td>
      <td><code>jj branch list</code></td>
      <td><code>git branch</code></td>
    </tr>
    <tr>
      <td>Create a branch</td>
      <td><code>jj branch create &lt;name&gt; -r &lt;revision&gt;</code></td>
      <td><code>git branch &lt;name&gt; &lt;revision&gt;</code></td>
    </tr>
    <tr>
      <td>Move a branch forward</td>
      <td><code>jj branch set &lt;name&gt; -r &lt;revision&gt;</code></td>
      <td><code>git branch -f &lt;name&gt; &lt;revision&gt;</code></td>
    </tr>
    <tr>
      <td>Move a branch backward or sideways</td>
      <td><code>jj branch set &lt;name&gt; -r &lt;revision&gt; --allow-backwards</code></td>
      <td><code>git branch -f &lt;name&gt; &lt;revision&gt;</code></td>
    </tr>
    <tr>
      <td>Delete a branch</td>
      <td><code>jj branch delete &lt;name&gt; </code></td>
      <td><code>git branch --delete &lt;name&gt;</code></td>
    </tr>
    <tr>
      <td>See log of operations performed on the repo</td>
      <td><code>jj op log</code></td>
      <td>Not supported</td>
    </tr>
    <tr>
      <td>Undo an earlier operation</td>
      <td><code>jj [op] undo &lt;operation ID&gt;</code>
          (<code>jj undo</code> is an alias for <code>jj op undo</code>)
      </td>
      <td>Not supported</td>
    </tr>
    <tr>
      <td>Create a commit that cancels out a previous commit</td>
      <td><code>jj backout -r &lt;revision&gt;</code>
      </td>
      <td><code>git revert &lt;revision&gt;</code></td>
    </tr>
  </tbody>
</table>
