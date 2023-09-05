# Revsets

Jujutsu supports a functional language for selecting a set of revisions.
Expressions in this language are called "revsets" (the idea comes from
[Mercurial](https://www.mercurial-scm.org/repo/hg/help/revsets)). The language
consists of symbols, operators, and functions.

Most `jj` commands accept a revset (or multiple). Many commands, such as
`jj diff -r <revset>` expect the revset to resolve to a single commit; it is
an error to pass a revset that resolves to more than one commit (or zero
commits) to such commands.

The words "revisions" and "commits" are used interchangeably in this document.

The commits listed by `jj log` without arguments are called "visible commits".
Other commits are only included if you explicitly mention them (e.g. by commit
ID or a Git ref pointing to them).

## Symbols

The `@` expression refers to the working copy commit in the current workspace.
Use `<workspace name>@` to refer to the working-copy commit in another
workspace. Use `<name>@<remote>` to refer to a remote-tracking branch.

A full commit ID refers to a single commit. A unique prefix of the full commit
ID can also be used. It is an error to use a non-unique prefix.

A full change ID refers to all visible commits with that change ID (there is
typically only one visible commit with a given change ID). A unique prefix of
the full change ID can also be used. It is an error to use a non-unique prefix.

Use double quotes to prevent a symbol from being interpreted as an expression.
For example, `"x-"` is the symbol `x-`, not the parents of symbol `x`.
Taking shell quoting into account, you may need to use something like
`jj log -r '"x-"'`.

### Priority

Jujutsu attempts to resolve a symbol in the following order:

1. Tag name
2. Branch name
3. Git ref
4. Commit ID or change ID

## Operators

The following operators are supported. `x` and `y` below can be any revset, not
only symbols.

* `x & y`: Revisions that are in both `x` and `y`.
* `x | y`: Revisions that are in either `x` or `y` (or both).
* `x ~ y`: Revisions that are in `x` but not in `y`.
* `~x`: Revisions that are not in `x`.
* `x-`: Parents of `x`.
* `x+`: Children of `x`.
* `::x`: Ancestors of `x`, including the commits in `x` itself.
* `x::`: Descendants of `x`, including the commits in `x` itself.
* `x::y`: Descendants of `x` that are also ancestors of `y`. Equivalent
   to `x:: & ::y`. This is what `git log` calls `--ancestry-path x..y`.
* `::`: All visible commits in the repo. Equivalent to `all()`.
* `:x`, `x:`, and `x:y`: Deprecated versions of `::x`, `x::`, and `x::y` We
  plan to delete them in jj 0.15+.
* `x..y`: Ancestors of `y` that are not also ancestors of `x`. Equivalent to
  `::y ~ ::x`. This is what `git log` calls `x..y` (i.e. the same as we call it).
* `..x`: Ancestors of `x`, including the commits in `x` itself, but excluding
  the root commit. Equivalent to `::x ~ root()`.
* `x..`: Revisions that are not ancestors of `x`.
* `..`: All visible commits in the repo, but excluding the root commit.
  Equivalent to `~root()`.

You can use parentheses to control evaluation order, such as `(x & y) | z` or
`x & (y | z)`.

## Functions

You can also specify revisions by using functions. Some functions take other
revsets (expressions) as arguments.

* `parents(x)`: Same as `x-`.
* `children(x)`: Same as `x+`.
* `ancestors(x)`: Same as `:x`.
* `descendants(x)`: Same as `x::`.
* `connected(x)`: Same as `x::x`. Useful when `x` includes several commits.
* `all()`: All visible commits in the repo.
* `none()`: No commits. This function is rarely useful; it is provided for
  completeness.
* `branches([pattern])`: All local branch targets. If `pattern` is specified,
  branches whose name contains the given string are selected. For example,
  `branches(push)` would match the branches `push-123` and `repushed` but not
  the branch `main`. If a branch is in a conflicted state, all its possible
  targets are included.
* `remote_branches([branch_pattern[, [remote=]remote_pattern]])`: All remote
  branch targets across all remotes. If just the `branch_pattern` is specified,
  branches whose name contains the given string across all remotes are
  selected. If both `branch_pattern` and `remote_pattern` are specified, the
  selection is further restricted to just the remotes whose name contains
  `remote_pattern`. For example, `remote_branches(push, ri)` would match the
  branches `push-123@origin` and `repushed@private` but not `push-123@upstream`
  or `main@origin` or `main@upstream`. If a branch is in a conflicted state,
  all its possible targets are included.
* `tags()`: All tag targets. If a tag is in a conflicted state, all its
  possible targets are included.
* `git_refs()`:  All Git ref targets as of the last import. If a Git ref
  is in a conflicted state, all its possible targets are included.
* `git_head()`: The Git `HEAD` target as of the last import. Equivalent to
  `present(HEAD@git)`.
* `visible_heads()`: All visible heads (same as `heads(all())`).
* `root()`: The virtual commit that is the oldest ancestor of all other commits.
* `heads(x)`: Commits in `x` that are not ancestors of other commits in `x`.
  Note that this is different from
  [Mercurial's](https://repo.mercurial-scm.org/hg/help/revsets) `heads(x)`
  function, which is equivalent to `x ~ x-`.
* `roots(x)`: Commits in `x` that are not descendants of other commits in `x`.
  Note that this is different from
  [Mercurial's](https://repo.mercurial-scm.org/hg/help/revsets) `roots(x)`
  function, which is equivalent to `x ~ x+`.
* `latest(x[, count])`: Latest `count` commits in `x`, based on committer
  timestamp. The default `count` is 1.
* `merges()`: Merge commits.
* `description(pattern)`: Commits with the given string in their
  description.
* `author(pattern)`: Commits with the given string in the author's name or
  email.
* `mine()`: Commits where the author's email matches the email of the current
  user.
* `committer(pattern)`: Commits with the given string in the committer's
  name or email.
* `empty()`: Commits modifying no files. This also includes `merges()` without
  user modifications and `root()`.
* `file(pattern..)`: Commits modifying the paths specified by the `pattern..`.
  Paths are relative to the directory `jj` was invoked from. A directory name
  will match all files in that directory and its subdirectories. For example,
  `file(foo)` will match files `foo`, `foo/bar`, `foo/bar/baz`, but not file
  `foobar`.
* `conflict()`: Commits with conflicts.
* `present(x)`: Same as `x`, but evaluated to `none()` if any of the commits
  in `x` doesn't exist (e.g. is an unknown branch name.)

## String patterns

Functions that perform string matching support the following pattern syntax.

* `"string"`, `substring:"string"`: Matches strings that contain `string`.
* `exact:"string"`: Matches strings exactly equal to `string`.

## Aliases

New symbols and functions can be defined in the config file, by using any
combination of the predefined symbols/functions and other aliases.

For example:

```toml
[revset-aliases]
'mine' = 'author(martinvonz)'
'user(x)' = 'author(x) | committer(x)'
```

## Examples

Show the parent(s) of the working-copy commit (like `git log -1 HEAD`):

```
jj log -r @-
```

Show commits not on any remote branch:

```
jj log -r 'remote_branches()..'
```

Show commits not on `origin` (if you have other remotes like `fork`):

```
jj log -r 'remote_branches(remote=origin)..'
```

Show all ancestors of the working copy (almost like plain `git log`)

```
jj log -r ::@
```

Show the initial commits in the repo (the ones Git calls "root commits"):

```
jj log -r root()+
```

Show some important commits (like `git --simplify-by-decoration`):

```
jj log -r 'tags() | branches()'
```

Show local commits leading up to the working copy, as well as descendants of
those commits:


```
jj log -r '(remote_branches()..@)::'
```

Show commits authored by "martinvonz" and containing the word "reset" in the
description:

```
jj log -r 'author(martinvonz) & description(reset)'
```
