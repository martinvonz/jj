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

The symbol `root` refers to the virtual commit that is the oldest ancestor of
all other commits.

The symbol `@` refers to the working copy commit in the current workspace (
Jujutsu supports only one workspace per repo
[so far](https://github.com/martinvonz/jj/issues/13)).

A full commit ID refers to a single commit. A unique prefix of the full commit
ID can also be used. It is an error to use a non-unique prefix.

A full change ID refers to all visible commits with that change ID (there is
typically only one visible commit with a given change ID). A unique prefix of
the full change ID can also be used. It is an error to use a non-unique prefix.

Use double quotes to prevent a symbol from being interpreted as an expression.
For example, `"x-1"` is the symbol `x-1`, not the parents of symbol `x`.
Taking shell quoting into account, you may need to use something like
`jj log -r '"x-1"'`.

### Priority

Jujutsu attempts to resolve a symbol in the following order:

1. `@`
2. `root`
3. Tag name
4. Branch name
5. Git ref
6. Commit ID
7. Change ID


## Operators

The following operators are supported. `x` and `y` below can be any revset, not
only symbols.

* `x & y`: Revisions that are in both `x` and `y`.
* `x | y`: Revisions that are in either `x` or `y` (or both).
* `x ~ y`: Revisions that are in `x` but not in `y`.
* `x-`: Parents of `x`.
* `x+`: Children of `x`.
* `:x`: Ancestors of `x`, including the commits in `x` itself.
* `x:`: Descendants of `x`, including the commits in `x` itself.
* `x:y`: Descendants of `x` that are also ancestors of `y`, both inclusive.
  Equivalent to `x: & :y`. This is what `git log` calls `--ancestry-path x..y`.
* `x..y`: Ancestors of `y` that are not also ancestors of `x`, both inclusive.
  Equivalent to `:y ~ :x`. This is what `git log` calls `x..y` (i.e. the same as
  we call it).
* `..x`: Ancestors of `x`, including the commits in `x` itself. Equivalent to
   `:x` and provided for consistency.
* `x..`: Revisions that are not ancestors of `x`.

You can use parentheses to control evaluation order, such as `(x & y) | z` or
`x & (y | z)`.


## Functions

You can also specify revisions by using functions. Some functions take other
revsets (expressions) as arguments.

* `parents(x)`: Same as `x-`.
* `children(x)`: Same as `x+`.
* `ancestors(x)`: Same as `:x`.
* `descendants(x)`: Same as `x:`.
* `connected(x)`: Same as `x:x`.
* `all()`: All visible commits in the repo.
* `none()`: No commits. This function is rarely useful; it is provided for
  completeness.
* `branches()`: All local branch targets. If a branch is in a conflicted state,
  all its possible targets are included.
* `remote_branches()`: All remote branch targets across all remotes. If a
  branch is in a conflicted state, all its possible targets are included.
* `tags()`: All tag targets. If a tag is in a conflicted state, all its
  possible targets are included.
* `git_refs()`:  All Git ref targets as of the last import. If a Git ref
  is in a conflicted state, all its possible targets are included.
* `git_head()`: The Git `HEAD` target as of the last import.
* `heads([x])`: Commits in `x` that are not ancestors of other commits in `x`.
  If `x` was not specified, it selects all visible heads (as if you had said
  `heads(all())`).
* `merges([x])`: Merge commits within `x`. If `x` was not specified, it selects
  all visible merge commits (as if you had said `merges(all())`).
* `description(needle[, x])`: Commits with the given string in their
  description. If a second argument was provided, then only commits in that set
  are considered, otherwise all visible commits are considered.
* `author(needle[, x])`: Commits with the given string in the author's name or
  email. If a second argument was provided, then only commits in that set
  are considered, otherwise all visible commits are considered.
* `committer(needle[, x])`: Commits with the given string in the committer's
  name or email. If a second argument was provided, then only commits in that
  set are considered, otherwise all visible commits are considered.


## Examples

Show the parent(s) of the working copy commit (like `git log -1 HEAD`):
```
jj log -r @-
```

Show commits not on any remote branch:
```
jj log -r 'remote_branches()..'
```

Show all ancestors of the working copy (almost like plain `git log`)
```
jj log -r :@
```

Show the initial commits in the repo (the ones Git calls "root commits"):
```
jj log -r root+
```

Show some important commits (like `git --simplify-by-decoration`):
```
jj log -r 'tags() | branches()'
```

Show local commits leading up to the working copy, as well as descendants of
those commits:
```
jj log -r '(remote_branches()..@):'
```

Show commits authored by "martinvonz" and containing the word "reset" in the
description:
```
jj log -r 'author(martinvonz) & description(reset)'
```
