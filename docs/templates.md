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

In `jj log`/`jj obslog` templates, all 0-argument methods of [the `Commit`
type](#commit-type) are available as keywords. For example, `commit_id` is
equivalent to `self.commit_id()`.

### Operation keywords

In `jj op log` templates, all 0-argument methods of [the `Operation`
type](#operation-type) are available as keywords. For example,
`current_operation` is equivalent to `self.current_operation()`.

## Operators

The following operators are supported.

* `x.f()`: Method call.
* `-x`: Negate integer value.
* `!x`: Logical not.
* `x && y`: Logical and, short-circuiting.
* `x || y`: Logical or, short-circuiting.
* `x ++ y`: Concatenate `x` and `y` templates.

## Global functions

The following functions are defined.

* `fill(width: Integer, content: Template) -> Template`: Fill lines at
  the given `width`.
* `indent(prefix: Template, content: Template) -> Template`: Indent
  non-empty lines by the given `prefix`.
* `label(label: Template, content: Template) -> Template`: Apply label to
  the content. The `label` is evaluated as a space-separated string.
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
* `working_copies() -> String`: For multi-workspace repository, indicate
  working-copy commit as `<workspace name>@`.
* `current_working_copy() -> Boolean`: True for the working-copy commit of the
  current workspace.
* `branches() -> List<RefName>`: Local and remote branches pointing to the commit.
  A tracking remote branch will be included only if its target is different
  from the local one.
* `local_branches() -> List<RefName>`: All local branches pointing to the commit.
* `remote_branches() -> List<RefName>`: All remote branches pointing to the commit.
* `tags() -> List<RefName>`
* `git_refs() -> List<RefName>`
* `git_head() -> Option<RefName>`
* `divergent() -> Boolean`: True if the commit's change id corresponds to multiple
  visible commits.
* `hidden() -> Boolean`: True if the commit is not visible (a.k.a. abandoned).
* `immutable() -> Boolean`: True if the commit is included in [the set of
  immutable commits](config.md#set-of-immutable-commits).
* `conflict() -> Boolean`: True if the commit contains merge conflicts.
* `empty() -> Boolean`: True if the commit modifies no files.
* `root() -> Boolean`: True if the commit is the root commit.

### CommitId / ChangeId type

The following methods are defined.

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
* `root() -> Boolean`: True if the commit is the root commit.

### OperationId type

The following methods are defined.

* `.short([len: Integer]) -> String`

### Option type

An option can be implicitly converted to `Boolean` denoting whether the
contained value is set. If set, all methods of the contained value can be
invoked. If not set, an error will be reported inline on method call.

### RefName type

The following methods are defined.

* `.name() -> String`: Local branch or tag name.
* `.remote() -> String`: Remote name or empty if this is a local ref.

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

String literals must be surrounded by double quotes (`"`). The following escape
sequences starting with a backslash have their usual meaning: `\"`, `\\`, `\n`,
`\r`, `\t`, `\0`. Other escape sequences are not supported. Any UTF-8 characters
are allowed inside a string literal, with two exceptions: unescaped `"`-s and
uses of `\` that don't form a valid escape sequence.

### Template type

Most types can be implicitly converted to `Template`. No methods are defined.

### Timestamp type

The following methods are defined.

* `.ago() -> String`: Format as relative timestamp.
* `.format(format: String) -> String`: Format with [the specified strftime-like
  format string](https://docs.rs/chrono/latest/chrono/format/strftime/).
* `.utc() -> Timestamp`: Convert timestamp into UTC timezone.
* `.local() -> Timestamp`: Convert timestamp into local timezone.

### TimestampRange type

The following methods are defined.

* `.start() -> Timestamp`
* `.end() -> Timestamp`
* `.duration() -> String`

## Configuration

The default templates and aliases() are defined in the `[templates]` and
`[template-aliases]` sections of the config respectively. The exact definitions
can be seen in the `cli/src/config/templates.toml` file in jj's source tree.

<!--- TODO: Find a way to embed the default config files in the docs -->

New keywords and functions can be defined as aliases, by using any
combination of the predefined keywords/functions and other aliases.

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
