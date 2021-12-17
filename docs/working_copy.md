# Working copy


## Introduction

The working copy is where the current checkout's files are written so you can
interact with them. It also where files are read from in order to create new
commits (though there are many other ways of creating new commits).

Unlike most other VCSs, Jujutsu will automatically create commits from the
working copy contents when they have changed. Most `jj` commands you run will
commit the working copy changes if they have changed. The resulting revision
will replace the previous working copy revision.

Also unlike most other VCSs, added files are implicitly tracked. That means that
if you add a new file to the working copy, it will be automatically committed
once you run e.g. `jj st`. Similarly, if you remove a file from the working
copy, it will implicitly be untracked. There is no easy way to make it untrack
already tracked files (https://github.com/martinvonz/jj/issues/14).

Jujutsu currently supports only one working copy
(https://github.com/martinvonz/jj/issues/13).


## Open/closed revisions

As described in the introduction, Jujutsu automatically rewrites the current
checkout with any changes from the working copy. That works well while you're
developing that revision. On the other hand, if you check out some existing
revision, you generally don't want changes to the working copy to automatically
rewrite that revision. Jujutsu has a concept of "open" and "closed" revisions to
solve this. When you check out a closed revision, Jujutsu will actually create a
new, *open* revision on top of it and check that out. The checked-out revision
is thus always open. When you are done making changes to the currently
checked-out revision, you close it by running `jj close`. That command then
updates to the rewritten revision (as most `jj` commands do), and since the
rewritten revision is now closed, it creates a new open revision on top. If you
check out a closed revision and make changes on top of it that you want to go
into the revision, use `jj squash`.


## Conflicts

The working copy cannot contain conflicts. When you check out a revision that
has conflicts, Jujutsu creates a new revision on top with the conflicts
"materialized" as regular files. That revision will then be what's actually
checked out. Materialized conflicts are simply files where the conflicting
regions have been replaced by conflict markers.

Once you have resolved the conflicts, use `jj squash` to move the conflict
resolutions into the conflicted revision.

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
