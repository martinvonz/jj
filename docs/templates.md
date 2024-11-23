# Templates

Jujutsu supports a functional language to customize output of commands.
The language consists of literals, keywords, operators, functions, and
methods.

A couple of `jj` commands accept a template via `-T`/`--template` option.

## Keywords

Keywords represent objects of different types; the types are described in
a follow-up section. In addition to context-specific keywords, the top-level
object can be referenced as `self`.

### Commit keywords

In `jj log`/`jj evolog` templates, all 0-argument methods of [the `Commit`
type](#commit-type) are available as keywords. For example, `commit_id` is
equivalent to `self.commit_id()`.

 * `description: String`
 * `change_id: ChangeId`
 * `commit_id: CommitId`
 * `parents: List<Commit>`
 * `author: Signature`
 * `committer: Signature`
 * `signature: CommitSignature`: The information about a cryptographic signature of the commit.
 * `working_copies: String`: For multi-workspace repository, indicate
   working-copy commit as `<workspace name>@`.
 * `current_working_copy: Boolean`: True for the working-copy commit of the
   current workspace.
 * `branches: List<RefName>`: Local and remote branches pointing to the commit.
   A tracking remote branch will be included only if its target is different
   from the local one.
 * `local_branches: List<RefName>`: All local branches pointing to the commit.
 * `remote_branches: List<RefName>`: All remote branches pointing to the commit.
 * `tags: List<RefName>`
 * `git_refs: List<RefName>`
 * `git_head: List<RefName>`
 * `divergent: Boolean`: True if the commit's change id corresponds to multiple
   visible commits.
 * `hidden: Boolean`: True if the commit is not visible (a.k.a. abandoned).
 * `conflict: Boolean`: True if the commit contains merge conflicts.
 * `empty: Boolean`: True if the commit modifies no files.
 * `root: Boolean`: True if the commit is the root commit.

### Operation keywords

In `jj op log` templates, all 0-argument methods of [the `Operation`
type](#operation-type) are available as keywords. For example,
`current_operation` is equivalent to `self.current_operation()`.

## Operators

The following operators are supported.

* `x.f()`: Method call.
* `-x`: Negate integer value.
* `!x`: Logical not.
* `x == y`, `x != y`: Logical equal/not equal. Operands must be either
  `Boolean`, `Integer`, or `String`.
* `x && y`: Logical and, short-circuiting.
* `x || y`: Logical or, short-circuiting.
* `x ++ y`: Concatenate `x` and `y` templates.

(listed in order of binding strengths)

## Global functions

The following functions are defined.

* `fill(width: Integer, content: Template) -> Template`: Fill lines at
  the given `width`.
* `indent(prefix: Template, content: Template) -> Template`: Indent
  non-empty lines by the given `prefix`.
* `pad_start(width: Integer, content: Template[, fill_char: Template])`: Pad (or
  right-justify) content by adding leading fill characters. The `content`
  shouldn't have newline character.
* `pad_end(width: Integer, content: Template[, fill_char: Template])`: Pad (or
  left-justify) content by adding trailing fill characters. The `content`
  shouldn't have newline character.
* `truncate_start(width: Integer, content: Template)`: Truncate `content` by
  removing leading characters. The `content` shouldn't have newline character.
* `truncate_end(width: Integer, content: Template)`: Truncate `content` by
  removing trailing characters. The `content` shouldn't have newline character.
* `label(label: Template, content: Template) -> Template`: Apply label to
  the content. The `label` is evaluated as a space-separated string.
* `raw_escape_sequence(content: Template) -> Template`: Preserves any escape
  sequences in `content` (i.e., bypasses sanitization) and strips labels.
  Note: This function is intended for escape sequences and as such, its output
  is expected to be invisible / of no display width. Outputting content with
  nonzero display width may break wrapping, indentation etc.
* `if(condition: Boolean, then: Template[, else: Template]) -> Template`:
  Conditionally evaluate `then`/`else` template content.
* `coalesce(content: Template...) -> Template`: Returns the first **non-empty**
  content.
* `concat(content: Template...) -> Template`:
  Same as `content_1 ++ ... ++ content_n`.
* `separate(separator: Template, content: Template...) -> Template`:
  Insert separator between **non-empty** contents.
* `surround(prefix: Template, suffix: Template, content: Template) -> Template`:
  Surround **non-empty** content with texts such as parentheses.

## Types

### Boolean type

No methods are defined. Can be constructed with `false` or `true` literal.

### Commit type

This type cannot be printed. The following methods are defined.

* `description() -> String`
* `change_id() -> ChangeId`
* `commit_id() -> CommitId`
* `parents() -> List<Commit>`
* `author() -> Signature`
* `committer() -> Signature`
* `mine() -> Boolean`: Commits where the author's email matches the email of the current
  user.
* `working_copies() -> String`: For multi-workspace repository, indicate
  working-copy commit as `<workspace name>@`.
* `current_working_copy() -> Boolean`: True for the working-copy commit of the
  current workspace.
* `bookmarks() -> List<RefName>`: Local and remote bookmarks pointing to the
  commit. A tracking remote bookmark will be included only if its target is
  different from the local one.
* `local_bookmarks() -> List<RefName>`: All local bookmarks pointing to the commit.
* `remote_bookmarks() -> List<RefName>`: All remote bookmarks pointing to the commit.
* `tags() -> List<RefName>`
* `git_refs() -> List<RefName>`
* `git_head() -> Boolean`: True for the Git `HEAD` commit.
* `divergent() -> Boolean`: True if the commit's change id corresponds to multiple
  visible commits.
* `hidden() -> Boolean`: True if the commit is not visible (a.k.a. abandoned).
* `immutable() -> Boolean`: True if the commit is included in [the set of
  immutable commits](config.md#set-of-immutable-commits).
* `contained_in(revset: String) -> Boolean`: True if the commit is included in [the provided revset](revsets.md).
* `conflict() -> Boolean`: True if the commit contains merge conflicts.
* `empty() -> Boolean`: True if the commit modifies no files.
* `diff([files: String]) -> TreeDiff`: Changes from the parents within [the
  `files` expression](filesets.md). All files are compared by default, but it is
  likely to change in future version to respect the command line path arguments.
* `root() -> Boolean`: True if the commit is the root commit.

### CommitId / ChangeId type

The following methods are defined.

* `.normal_hex() -> String`: Normal hex representation (0-9a-f), useful for
  ChangeId, whose canonical hex representation is "reversed" (z-k).
* `.short([len: Integer]) -> String`
* `.shortest([min_len: Integer]) -> ShortestIdPrefix`: Shortest unique prefix.

### Integer type

No methods are defined.

### List type

A list can be implicitly converted to `Boolean`. The following methods are
defined.

* `.len() -> Integer`: Number of elements in the list.
* `.join(separator: Template) -> Template`: Concatenate elements with
  the given `separator`.
* `.map(|item| expression) -> ListTemplate`: Apply template `expression`
  to each element. Example: `parents.map(|c| c.commit_id().short())`

### ListTemplate type

The following methods are defined. See also the `List` type.

* `.join(separator: Template) -> Template`

### Operation type

This type cannot be printed. The following methods are defined.

* `current_operation() -> Boolean`
* `description() -> String`
* `id() -> OperationId`
* `tags() -> String`
* `time() -> TimestampRange`
* `user() -> String`
* `snapshot() -> Boolean`: True if the operation is a snapshot operation.
* `root() -> Boolean`: True if the operation is the root operation.

### OperationId type

The following methods are defined.

* `.short([len: Integer]) -> String`

### Option type

An option can be implicitly converted to `Boolean` denoting whether the
contained value is set. If set, all methods of the contained value can be
invoked. If not set, an error will be reported inline on method call.

### RefName type

The following methods are defined.

* `.name() -> String`: Local bookmark or tag name.
* `.remote() -> String`: Remote name or empty if this is a local ref.
* `.present() -> Boolean`: True if the ref points to any commit.
* `.conflict() -> Boolean`: True if [the bookmark or tag is
  conflicted](bookmarks.md#conflicts).
* `.normal_target() -> Option<Commit>`: Target commit if the ref is not
  conflicted and points to a commit.
* `.removed_targets() -> List<Commit>`: Old target commits if conflicted.
* `.added_targets() -> List<Commit>`: New target commits. The list usually
  contains one "normal" target.
* `.tracked() -> Boolean`: True if the ref is tracked by a local ref. The local
  ref might have been deleted (but not pushed yet.)
* `.tracking_present() -> Boolean`: True if the ref is tracked by a local ref,
    and if the local ref points to any commit.
* `.tracking_ahead_count() -> SizeHint`: Number of commits ahead of the tracking
  local ref.
* `.tracking_behind_count() -> SizeHint`: Number of commits behind of the
  tracking local ref.

### ShortestIdPrefix type

The following methods are defined.

* `.prefix() -> String`
* `.rest() -> String`
* `.upper() -> ShortestIdPrefix`
* `.lower() -> ShortestIdPrefix`

### Signature type

The following methods are defined.

* `.name() -> String`
* `.email() -> String`
* `.username() -> String`
* `.timestamp() -> Timestamp`

### SizeHint type

This type cannot be printed. The following methods are defined.

* `.lower() -> Integer`: Lower bound.
* `.upper() -> Option<Integer>`: Upper bound if known.
* `.exact() -> Option<Integer>`: Exact value if upper bound is known and it
  equals to the lower bound.
* `.zero() -> Boolean`: True if upper bound is known and is `0`.

### CommitSignature type

The following methods are defined.

* `.present() -> Boolean`: True if the commit has a cryptographic signature.
* `.good() -> Boolean`: True if the signature matches the commit data.
* `.unknown() -> Boolean`: True if the signing backend cannot verify the signature (e.g. due to a missing public key), or if there's no backend implemented that can verify the signature.
* `.bad() -> Boolean`: True if the signature does not match the commit data.
* `.invalid() -> Boolean`: True if the signature is detected to be made with a signing backend (e.g. has a PGP prefix) but is otherwise invalid.
* `.key() -> String`: Signing backend specific key id. For GPG, it's a long key ID, present for all non-invalid signatures.
* `.display() -> String`: Signing backend specific display string. For GPG, it's a formatted primary user ID, only present if the public key is known (only for good/bad signatures).

### String type

A string can be implicitly converted to `Boolean`. The following methods are
defined.

* `.len() -> Integer`: Length in UTF-8 bytes.
* `.contains(needle: Template) -> Boolean`
* `.first_line() -> String`
* `.lines() -> List<String>`: Split into lines excluding newline characters.
* `.upper() -> String`
* `.lower() -> String`
* `.starts_with(needle: Template) -> Boolean`
* `.ends_with(needle: Template) -> Boolean`
* `.remove_prefix(needle: Template) -> String`: Removes the passed prefix, if present
* `.remove_suffix(needle: Template) -> String`: Removes the passed suffix, if present
* `.substr(start: Integer, end: Integer) -> String`: Extract substring. The
  `start`/`end` indices should be specified in UTF-8 bytes. Negative values
  count from the end of the string.

#### String literals

String literals must be surrounded by single or double quotes (`'` or `"`).
A double-quoted string literal supports the following escape sequences:

* `\"`: double quote
* `\\`: backslash
* `\t`: horizontal tab
* `\r`: carriage return
* `\n`: new line
* `\0`: null
* `\e`: escape (i.e., `\x1b`)
* `\xHH`: byte with hex value `HH`

Other escape sequences are not supported. Any UTF-8 characters are allowed
inside a string literal, with two exceptions: unescaped `"`-s and uses of `\`
that don't form a valid escape sequence.

A single-quoted string literal has no escape syntax. `'` can't be expressed
inside a single-quoted string literal.

### Template type

Most types can be implicitly converted to `Template`. No methods are defined.

### Timestamp type

The following methods are defined.

* `.ago() -> String`: Format as relative timestamp.
* `.format(format: String) -> String`: Format with [the specified strftime-like
  format string](https://docs.rs/chrono/latest/chrono/format/strftime/).
* `.utc() -> Timestamp`: Convert timestamp into UTC timezone.
* `.local() -> Timestamp`: Convert timestamp into local timezone.
* `.after(date: String) -> Boolean`: True if the timestamp is exactly at or after the given date.
* `.before(date: String) -> Boolean`: True if the timestamp is before, but not including, the given date.

### TimestampRange type

The following methods are defined.

* `.start() -> Timestamp`
* `.end() -> Timestamp`
* `.duration() -> String`

### TreeDiff type

This type cannot be printed. The following methods are defined.

* `.color_words([context: Integer]) -> Template`: Format as a word-level diff
  with changes indicated only by color.
* `.git([context: Integer]) -> Template`: Format as a Git diff.
* `.stat(width: Integer) -> Template`: Format as a histogram of the changes.
* `.summary() -> Template`: Format as a list of status code and path pairs.

## Configuration

The default templates and aliases() are defined in the `[templates]` and
`[template-aliases]` sections of the config respectively. The exact definitions
can be seen in the `cli/src/config/templates.toml` file in jj's source tree.

<!--- TODO: Find a way to embed the default config files in the docs -->

New keywords and functions can be defined as aliases, by using any
combination of the predefined keywords/functions and other aliases.

Alias functions can be overloaded by the number of parameters. However, builtin
function will be shadowed by name, and can't co-exist with aliases.

For example:

```toml
[template-aliases]
'commit_change_ids' = '''
concat(
  format_field("Commit ID", commit_id),
  format_field("Change ID", commit_id),
)
'''
'format_field(key, value)' = 'key ++ ": " ++ value ++ "\n"'
```

## Examples

Get short commit IDs of the working-copy parents:

```sh
jj log --no-graph -r @ -T 'parents.map(|c| c.commit_id().short()).join(",")'
```

Show machine-readable list of full commit and change IDs:

```sh
jj log --no-graph -T 'commit_id ++ " " ++ change_id ++ "\n"'
```
