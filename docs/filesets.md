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

Many `jj` commands accept fileset expressions as positional arguments. File
names passed to these commands [must be quoted][string-literals] if they contain
whitespace or meta characters. However, as a special case, quotes can be omitted
if the expression has no operators nor function calls. For example:

- `jj diff 'Foo Bar'` (shell quotes are required, but inner quotes are optional)
- `jj diff '~"Foo Bar"'` (both shell and inner quotes are required)
- `jj diff '"Foo(1)"'` (both shell and inner quotes are required)

[string-literals]: templates.md#string-literals

## File patterns

The following patterns are supported:

- `"path"`, `path` (the quotes are optional), or `cwd:"path"`: Matches
  cwd-relative path prefix (file or files under directory recursively.)
- `cwd-file:"path"` or `file:"path"`: Matches cwd-relative file (or exact) path.
- `cwd-glob:"pattern"` or `glob:"pattern"`: Matches file paths with cwd-relative
  Unix-style shell [wildcard `pattern`][glob]. For example, `glob:"*.c"` will
  match all `.c` files in the current working directory non-recursively.
- `root:"path"`: Matches workspace-relative path prefix (file or files under
  directory recursively.)
- `root-file:"path"`: Matches workspace-relative file (or exact) path.
- `root-glob:"pattern"`: Matches file paths with workspace-relative Unix-style
  shell [wildcard `pattern`][glob].

[glob]: https://docs.rs/glob/latest/glob/struct.Pattern.html

## Operators

The following operators are supported. `x` and `y` below can be any fileset
expressions.

- `x & y`: Matches both `x` and `y`.
- `x | y`: Matches either `x` or `y` (or both).
- `x ~ y`: Matches `x` but not `y`.
- `~x`: Matches everything but `x`.

You can use parentheses to control evaluation order, such as `(x & y) | z` or
`x & (y | z)`.

## Functions

You can also specify patterns by using functions.

- `all()`: Matches everything.
- `none()`: Matches nothing.

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
