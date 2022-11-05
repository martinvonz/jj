# Working copy


## Introduction

The working copy is where the current working-copy commit's files are written so
you can interact with them. It also where files are read from in order to create
new commits (though there are many other ways of creating new commits).

Unlike most other VCSs, Jujutsu will automatically create commits from the
working-copy contents when they have changed. Most `jj` commands you run will
commit the working-copy changes if they have changed. The resulting revision
will replace the previous working-copy revision.

Also unlike most other VCSs, added files are implicitly tracked. That means that
if you add a new file to the working copy, it will be automatically committed
once you run e.g. `jj st`. Similarly, if you remove a file from the working
copy, it will implicitly be untracked. To untrack a file while keeping it in
the working copy, first make sure it's [ignored](#ignored-files) and then run
`jj untrack <path>`.


## Open/closed revisions

This section only applies if you have set `ui.enable-open-commits = true`
in your config.

As described in the introduction, Jujutsu automatically rewrites the current
working-copy commit with any changes from the working copy. That works well
while you're developing that revision. On the other hand, if you check out some
existing revision, you generally don't want changes to the working copy to
automatically rewrite that revision. Jujutsu has a concept of "open" and
"closed" revisions to solve this. When you check out a closed revision, Jujutsu
will actually create a new, *open* revision on top of it and check that out. The
working-copy commit is thus always open. When you are done making changes to
the current working-copy commit, you close it by running `jj close`. That
command then updates to the rewritten revision (as most `jj` commands do), and
since the rewritten revision is now closed, it creates a new open revision on
top. If you check out a closed revision and make changes on top of it that you
want to go into the revision, use `jj squash`.


## Conflicts

When you check out a commit with conflicts, those conflicts need to be
represented in the working copy somehow. However, the file system doesn't
understand conflicts. Jujutsu's solution is to add conflict markers to
conflicted files when it writes them to the working copy. It also keeps track of
the (typically 3) different parts involved in the conflict. Whenever it scans
the working copy thereafter, it parses the conflict markers and recreates the
conflict state from them. You can resolve conflicts by replacing the conflict
markers by the resolved text. You don't need to resolve all conflicts at once.
You can even resolve part of a conflict by updating the different parts of the
conflict marker.

To resolve conflicts in a commit, use `jj new <commit>` to create a working-copy
commit on top. You would then have the same conflicts in the working-copy
commit. Once you have resolved the conflicts, you can inspect the conflict
resolutions with `jj diff`. Then run `jj squash` to move the conflict
resolutions into the conflicted commit. Alternatively, you can edit the commit
with conflicts directly in the working copy by using `jj edit <commit>`. The
main disadvantage of that is that it's harder to inspect the conflict
resolutions.

There's not yet a way of resolving conflicts in an external merge tool
(https://github.com/martinvonz/jj/issues/18). There's also no good way of
resolving conflicts between directories, files, and symlinks
(https://github.com/martinvonz/jj/issues/19). You can use `jj restore` to
choose one side of the conflict, but there's no way to even see where the
involved parts came from.


## Ignored files

You probably don't want build outputs and temporary files to be under version
control. You can tell Jujutsu to not automatically track certain files by using
`.gitignore` files (there's no such thing as `.jjignore` yet).
See https://git-scm.com/docs/gitignore for details about the format.
`.gitignore` files are supported in any directory in the working copy, as well
as in `$HOME/.gitignore`. However, `$GIT_DIR/info/exclude` or equivalent way
(maybe `.jj/gitignore`) of specifying per-clone ignores is not yet supported.


## Workspaces

You can have multiple working copies backed by a single repo. Use 
`jj workspace add` to create a new working copy. The working copy will have a
`.jj/` directory linked to the main repo. The working copy and the `.jj/`
directory together is called a "workspace". Each workspace can have a different
commit checked out.

Having multiple workspaces can be useful for running long-running tests in a one
while you continue developing in another, for example.

When you're done using a workspace, use `jj workspace forget` to make the repo
forget about it. The files can be deleted from disk separately (either before or
after).
