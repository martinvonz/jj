# Filesets

<!--
TODO: implement fileset parser and add logical operators

Jujutsu supports a functional language for selecting a set of files.
Expressions in this language are called "filesets" (the idea comes from
[Mercurial](https://repo.mercurial-scm.org/hg/help/filesets)). The language
consists of symbols, operators, and functions.
-->

## File patterns

The following patterns are supported:

* `"path"`, `path` (the quotes are optional), or `cwd:"path"`: Matches
  cwd-relative path prefix (file or files under directory recursively.)
* `cwd-file:"path"` or `file:"path"`: Matches cwd-relative file (or exact) path.
* `root:"path"`: Matches workspace-relative path prefix (file or files under
  directory recursively.)
* `root-file:"path"`: Matches workspace-relative file (or exact) path.
