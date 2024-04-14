# Filesets

Jujutsu supports a functional language for selecting a set of files.
Expressions in this language are called "filesets" (the idea comes from
[Mercurial](https://repo.mercurial-scm.org/hg/help/filesets)). The language
consists of file patterns, operators, and functions.

**Filesets support is still experimental.** It can be enabled by
`ui.allow-filesets`.

```toml
ui.allow-filesets = true
```

## File patterns

The following patterns are supported:

* `"path"`, `path` (the quotes are optional), or `cwd:"path"`: Matches
  cwd-relative path prefix (file or files under directory recursively.)
* `cwd-file:"path"` or `file:"path"`: Matches cwd-relative file (or exact) path.
* `cwd-glob:"pattern"` or `glob:"pattern"`: Matches file paths with cwd-relative
  Unix-style shell [wildcard `pattern`][glob]. For example, `glob:"*.c"` will
  match all `.c` files in the current working directory non-recursively.
* `root:"path"`: Matches workspace-relative path prefix (file or files under
  directory recursively.)
* `root-file:"path"`: Matches workspace-relative file (or exact) path.
* `root-glob:"pattern"`: Matches file paths with workspace-relative Unix-style
  shell [wildcard `pattern`][glob].

[glob]: https://docs.rs/glob/latest/glob/struct.Pattern.html

## Operators

The following operators are supported. `x` and `y` below can be any fileset
expressions.

* `x & y`: Matches both `x` and `y`.
* `x | y`: Matches either `x` or `y` (or both).
* `x ~ y`: Matches `x` but not `y`.
* `~x`: Matches everything but `x`.

You can use parentheses to control evaluation order, such as `(x & y) | z` or
`x & (y | z)`.

## Functions

You can also specify patterns by using functions.

* `all()`: Matches everything.
* `none()`: Matches nothing.

## Examples

Show diff excluding `Cargo.lock`.

```
jj diff '~Cargo.lock'
```

List files in `src` excluding Rust sources.

```
jj files 'src ~ glob:"**/*.rs"'
```

Split a revision in two, putting `foo` into the second commit.

```
jj split '~foo'
```
