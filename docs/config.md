# Configuration

These are the config settings available to jj/Jujutsu.


## Config files and TOML

`jj` loads several types of config settings:

- The built-in settings. These cannot be edited. They can be viewed in the
  `cli/src/config/` directory in `jj`'s source repo.

- The user settings. These can be edited with `jj config edit --user`. User
settings are located in [the user config file], which can be found with `jj
config path --user`.

- The repo settings. These can be edited with `jj config edit --repo` and are
located in `.jj/repo/config.toml`.

- Settings [specified in the command-line](#specifying-config-on-the-command-line).

These are listed in the order they are loaded; the settings from earlier items
in the list are overridden by the settings from later items if they disagree.
Every type of config except for the built-in settings is optional.

See the [TOML site] and the [syntax guide] for a detailed description of the
syntax. We cover some of the basics below.

[the user config file]: #user-config-file
[TOML site]: https://toml.io/en/
[syntax guide]: https://toml.io/en/v1.0.0

The first thing to remember is that the value of a setting (the part to the
right of the `=` sign) should be surrounded in quotes if it's a string.

### Dotted style, headings, and inline tables

In TOML, anything under a heading can be dotted instead. For example,
`user.name = "YOUR NAME"` is equivalent to:

```toml
[user]
name = "YOUR NAME"
```

For future reference, here are a couple of more complicated examples,

```toml
# Dotted style
template-aliases."format_short_id(id)" = "id.shortest(12)"
colors."commit_id prefix" = { bold = true }

# is equivalent to:
[template-aliases]
"format_short_id(id)" = "id.shortest(12)"

[colors]
"commit_id prefix" = { bold = true }
```

Dotted and non-inline table items are merged one by one if the same keys exist
in multiple files. In the example above, `template-aliases` and `colors` are
the tables to be merged. `template-aliases."format_short_id(id)"` and
`colors."commit_id prefix"` in the default settings are overridden. On the other
hand, inner items of an inline table are *not* merged. For example, `{ bold =
true }` wouldn't be merged as `{ fg = "blue", bold = true }` even if the default
settings had `colors."commit_id prefix" = { fg = "blue" }`.

The docs below refer to keys in text using dotted notation, but example
blocks will use heading notation to be unambiguous. If you are confident with TOML
then use whichever suits you in your config. If you mix dotted keys and headings,
**you must put the dotted keys before the first heading**.

That's probably enough TOML to keep you out of trouble but the [syntax guide] is
very short if you ever need to check.


## User settings

```toml
[user]
name = "YOUR NAME"
email = "YOUR_EMAIL@example.com"
```

Don't forget to change these to your own details!

## UI settings

### Colorizing output

Possible values are `always`, `never`, `debug` and `auto` (default: `auto`).
`auto` will use color only when writing to a terminal. `debug` will print the
active labels alongside the regular colorized output.

This setting overrides the `NO_COLOR` environment variable (if set).

```toml
[ui]
color = "never" # Turn off color
```

### Custom colors and styles

You can customize the colors used for various elements of the UI. For example:

```toml
[colors]
commit_id = "green"
```

The following colors are available:

* black
* red
* green
* yellow
* blue
* magenta
* cyan
* white
* default

All of them but "default" come in a bright version too, e.g. "bright red". The
"default" color can be used to override a color defined by a parent style
(explained below).

You can also use a 6-digit hex code for more control over the exact color used:

```toml
[colors]
change_id = "#ff1525"
```

If you use a string value for a color, as in the examples above, it will be used
for the foreground color. You can also set the background color, or make the
text bold or underlined. For that, you need to use a table:

```toml
[colors]
commit_id = { fg = "green", bg = "#ff1525", bold = true, underline = true }
```

The key names are called "labels". The above used `commit_id` as label. You can
also create rules combining multiple labels. The rules work a bit like CSS
selectors. For example, if you want to color commit IDs green in general but
make the commit ID of the working-copy commit also be underlined, you can do
this:

```toml
[colors]
commit_id = "green"
"working_copy commit_id" = { underline = true }
```

Parts of the style that are not overridden - such as the foreground color in the
example above - are inherited from the style of the parent label.

Which elements can be colored is not yet documented, but see
the [default color configuration](https://github.com/jj-vcs/jj/blob/main/cli/src/config/colors.toml)
for some examples of what's possible.

### Default command

When `jj` is run with no explicit subcommand, the value of the
`ui.default-command` setting will be used instead. Possible values are any valid
subcommand name, subcommand alias, or user-defined alias (defaults to `"log"`).

```toml
[ui]
default-command = ["log", "--reversed"]
```

### Default description

The editor content of a commit description can be populated by the
`draft_commit_description` template.

```toml
[templates]
draft_commit_description = '''
concat(
  description,
  surround(
    "\nJJ: This commit contains the following changes:\n", "",
    indent("JJ:     ", diff.stat(72)),
  ),
)
'''
```

The value of the `ui.default-description` setting can also be used in order to
fill in things like BUG=, TESTED= etc.

```toml
[ui]
default-description = "\n\nTESTED=TODO"
```

### Diff colors and styles

In color-words and git diffs, word-level hunks are rendered with underline. You
can override the default style with the following keys:

```toml
[colors]
# Highlight hunks with background
"diff removed token" = { bg = "#221111", underline = false }
"diff added token" = { bg = "#002200", underline = false }
```

### Diff format

```toml
[ui]
# Possible values: "color-words" (default), "git", "summary"
diff.format = "git"
```

#### Color-words diff options

In color-words diffs, changed words are displayed inline by default. Because
it's difficult to read a diff line with many removed/added words, there's a
threshold to switch to traditional separate-line format. You can also change
the default number of lines of context shown.

* `max-inline-alternation`: Maximum number of removed/added word alternation to
  inline. For example, `<added> ... <added>` sequence has 1 alternation, so the
  line will be inline if `max-inline-alternation >= 1`. `<added> ... <removed>
  ... <added>` sequence has 3 alternation.

  * `0`: disable inlining, making `--color-words` more similar to `--git`
  * `1`: inline removes-only or adds-only lines
  * `2`, `3`, ..: inline up to `2`, `3`, .. alternation
  * `-1`: inline all lines

  The default is `3`.

  **This parameter is experimental.** The definition is subject to change.
* `context`: Number of lines of context to show in the diff. The default is `3`.

```toml
[diff.color-words]
max-inline-alternation = 3
context = 3
```

#### Git diff options

In git diffs you can change the default number of lines of context shown.

* `context`: Number of lines of context to show in the diff. The default is `3`.

```toml
[diff.git]
context = 3
```

### Generating diffs by external command

If `ui.diff.tool` is set, the specified diff command will be called instead of
the internal diff function.

```toml
[ui]
# Use Difftastic by default
diff.tool = ["difft", "--color=always", "$left", "$right"]
# Use tool named "<name>" (see below)
diff.tool = "<name>"
```

The external diff tool can also be enabled by `diff --tool <name>` argument.
For the tool named `<name>`, command arguments can be configured as follows.

```toml
[merge-tools.<name>]
# program = "<name>"  # Defaults to the name of the tool if not specified
diff-args = ["--color=always", "$left", "$right"]
```

- `$left` and `$right` are replaced with the paths to the left and right
  directories to diff respectively.

By default `jj` will invoke external tools with a directory containing the left
and right sides.  The `diff-invocation-mode` config can change this to file by file
invocations as follows:

```toml
[ui]
diff.tool = "vimdiff"

[merge-tools.vimdiff]
diff-invocation-mode = "file-by-file"
```

### Conflict marker style

You can configure which style of conflict markers to use when materializing
conflicts:

```toml
[ui]
# Shows a single snapshot and one or more diffs to apply to it
conflict-marker-style = "diff"
# Shows a snapshot for each side and base of the conflict
conflict-marker-style = "snapshot"
# Uses Git's "diff3" conflict markers to support tools that depend on it
conflict-marker-style = "git"
```

For more details about these conflict marker styles, see the [conflicts
page](conflicts.md#conflict-markers).

### Set of immutable commits

You can configure the set of immutable commits via
`revset-aliases."immutable_heads()"`. The default set of immutable heads is
`builtin_immutable_heads()`, which in turn is defined as
`present(trunk()) | tags() | untracked_remote_bookmarks()`. For example, to
also consider the `release@origin` bookmark immutable:

```toml
[revset-aliases]
"immutable_heads()" = "builtin_immutable_heads() | release@origin"
```

To prevent rewriting commits authored by other users:

```toml
# The `trunk().. &` bit is an optimization to scan for non-`mine()` commits
# only among commits that are not in `trunk()`.
[revset-aliases]
"immutable_heads()" = "builtin_immutable_heads() | (trunk().. & ~mine())"
```

Ancestors of the configured set are also immutable. The root commit is always
immutable even if the set is empty.

## Log

### Default revisions

You can configure the revisions `jj log` would show when neither `-r` nor any paths are specified.

```toml
[revsets]
# Show commits that are not in `main@origin`
log = "main@origin.."
```

The default value for `revsets.log` is
`'present(@) | ancestors(immutable_heads().., 2) | present(trunk())'`.

### Default Template

You can configure the template used when no `-T` is specified.

- `templates.log` for `jj log`
- `templates.op_log` for `jj op log`
- `templates.show` for `jj show`

```toml
[templates]
# Use builtin log template
log = "builtin_log_compact"
# Use builtin op log template
op_log = "builtin_log_compact"
# Use builtin show template
show = "builtin_log_detailed"
```

If you want to see the full description when you do `jj log` you can add this to
your config:

```toml
[templates]
log = "builtin_log_compact_full_description"
```

### Graph style

```toml
[ui]
# Possible values: "curved" (default), "square", "ascii", "ascii-large"
graph.style = "square"
```

#### Node style

The symbols used to represent commits or operations can be customized via
templates.

- `templates.log_node` for commits (with `Option<Commit>` keywords)
- `templates.op_log_node` for operations (with `Operation` keywords)

For example:
```toml
[templates]
log_node = '''
coalesce(
  if(!self, "🮀"),
  if(current_working_copy, "@"),
  if(root, "┴"),
  if(immutable, "●", "○"),
)
'''
op_log_node = 'if(current_operation, "@", "○")'
```

### Wrap log content

If enabled, `log`/`evolog`/`op log` content will be wrapped based on
the terminal width.

```toml
[ui]
log-word-wrap = true
```

### Display of commit and change ids

Can be customized by the `format_short_id()` template alias.

```toml
[template-aliases]
# Highlight unique prefix and show at least 12 characters (default)
'format_short_id(id)' = 'id.shortest(12)'
# Just the shortest possible unique prefix
'format_short_id(id)' = 'id.shortest()'
# Show unique prefix and the rest surrounded by brackets
'format_short_id(id)' = 'id.shortest(12).prefix() ++ "[" ++ id.shortest(12).rest() ++ "]"'
# Always show 12 characters
'format_short_id(id)' = 'id.short(12)'
```

To customize these separately, use the `format_short_commit_id()` and
`format_short_change_id()` aliases:

```toml
[template-aliases]
# Uppercase change ids. `jj` treats change and commit ids as case-insensitive.
'format_short_change_id(id)' = 'format_short_id(id).upper()'
```

To get shorter prefixes for certain revisions, set `revsets.short-prefixes`:

```toml
[revsets]
# Prioritize the current bookmark
short-prefixes = "(main..@)::"
```

### Relative timestamps

Can be customized by the `format_timestamp()` template alias.

```toml
[template-aliases]
# Full timestamp in ISO 8601 format
'format_timestamp(timestamp)' = 'timestamp'
# Relative timestamp rendered as "x days/hours/seconds ago"
'format_timestamp(timestamp)' = 'timestamp.ago()'
```

`jj op log` defaults to relative timestamps. To use absolute timestamps, you
will need to modify the `format_time_range()` template alias.

```toml
[template-aliases]
'format_time_range(time_range)' = 'time_range.start() ++ " - " ++ time_range.end()'
```

### Author format

Can be customized by the `format_short_signature()` template alias.

```toml
[template-aliases]
# Full email address (default)
'format_short_signature(signature)' = 'signature.email()'
# Both name and email address
'format_short_signature(signature)' = 'signature'
# Username part of the email address
'format_short_signature(signature)' = 'signature.username()'
```

### Commit timestamp

Commits have both an "author timestamp" and "committer timestamp". By default,
jj displays the committer timestamp, but can be changed to show the author
timestamp instead.

The function must return a timestamp because the return value will likely be
formatted with `format_timestamp()`.

```toml
[template-aliases]
'commit_timestamp(commit)' = 'commit.author().timestamp()'
```

### Allow "large" revsets by default

Certain commands (such as `jj rebase`) can take multiple revset arguments, but
default to requiring each of those revsets to expand to a *single* revision.
This restriction can be overridden by prefixing a revset that the user wants to
be able to expand to more than one revision with the [`all:`
modifier](revsets.md#the-all-modifier).

Another way you can override this check is by setting
`ui.always-allow-large-revsets` to `true`. Then, `jj` will allow every one of
the revset arguments of such commands to expand to any number of revisions.

```toml
[ui]
# Assume `all:` prefix before revsets whenever it would make a difference
always-allow-large-revsets = true
```

## Pager

The default pager is can be set via `ui.pager` or the `PAGER` environment
variable. The priority is as follows (environment variables are marked with
a `$`):

`ui.pager` > `$PAGER`

`less -FRX` is the default pager in the absence of any other setting, except
on Windows where it is `:builtin`.

The special value `:builtin` enables usage of the [integrated pager called
`minus`](https://github.com/AMythicDev/minus/). See the [docs for the `minus`
pager](https://docs.rs/minus/latest/minus/#default-keybindings) for the key
bindings and some more details.

If you are using a standard Linux distro, your system likely already has
`$PAGER` set and that will be preferred over the built-in. To use the built-in:

```
jj config set --user ui.pager :builtin
```

It is possible the default will change to `:builtin` for all platforms in the
future.

Additionally, paging behavior can be toggled via `ui.paginate` like so:

```toml
[ui]
# Enable pagination for commands that support it (default)
paginate = "auto"
# Disable all pagination, equivalent to using --no-pager
paginate = "never"
```

### Processing contents to be paged

If you'd like to pass the output through a formatter e.g.
[`diff-so-fancy`](https://github.com/so-fancy/diff-so-fancy) before piping it
through a pager you must do it using a subshell as, unlike `git` or `hg`, the
command will be executed directly. For example:

```toml
[ui]
pager = ["sh", "-c", "diff-so-fancy | less -RFX"]
```

Some formatters (like [`delta`](https://github.com/dandavison/delta)) require
git style diffs for formatting. You can configure this style of
diff as the default with the `ui.diff` setting. For example:

```toml
[ui]
pager = "delta"

[ui.diff]
format = "git"
```

## Aliases

You can define aliases for commands, including their arguments. For example:

```toml
[aliases]
# `jj l` is a simple alias for `jj my-log`
l = "my-log"

# `jj my-log` shows commits on the working-copy commit's (anonymous) bookmark
# compared to the `main` bookmark
my-log = ["log", "-r", "(main..@):: | (main..@)-"]
```

This alias syntax can only run a single jj command. However, you may want to
execute multiple jj commands with a single alias, or run arbitrary scripts that
complement your version control workflow. This can be done, but be aware of the
danger:

!!! warning

    The following technique just provides a convenient syntax for running
    arbitrary code on your system. Using it irresponsibly may cause damage
    ranging from breaking the behavior of `jj undo` to wiping your file system.
    Exercise the same amount of caution while writing these aliases as you would
    when typing commands into the terminal!

    This feature may be removed or replaced by an embedded scripting language in
    the future.

The command `jj util exec` will simply run any command you pass to it as an
argument. Additional arguments are passed through. Here are some examples:

```toml
[aliases]
my-script = ["util", "exec", "--", "my-jj-script"]
#                            ^^^^
# This makes sure that flags are passed to your script instead of parsed by jj.
my-inline-script = ["util", "exec", "--", "bash", "-c", """
#!/usr/bin/env bash
set -euo pipefail
echo "Look Ma, everything in one file!"
echo "args: $@"
""", ""]
#    ^^
# This last empty string will become "$0" in bash, so your actual arguments
# are all included in "$@" and start at "$1" as expected.
```

## Editor

The default editor is set via `ui.editor`, though there are several places to
set it. The priority is as follows (environment variables are marked with
a `$`):

`$JJ_EDITOR` > `ui.editor` > `$VISUAL` > `$EDITOR`

Pico is the default editor (Notepad on Windows) in the absence of any other
setting, but you could set it explicitly too.

```toml
[ui]
editor = "pico"
```

To use NeoVim instead:

```toml
[ui]
editor = "nvim"
```

For GUI editors you possibly need to use a `-w` or `--wait`. Some examples:

```toml
[ui]
editor = "code -w"       # VS Code
editor = "code.cmd -w"   # VS Code on Windows
editor = "bbedit -w"     # BBEdit
editor = "subl -n -w"    # Sublime Text
editor = "mate -w"       # TextMate
editor = ["C:/Program Files/Notepad++/notepad++.exe",
    "-multiInst", "-notabbar", "-nosession", "-noPlugin"] # Notepad++
editor = "idea --temp-project --wait"   #IntelliJ
```

Obviously, you would only set one line, don't copy them all in!

## Editing diffs

The `ui.diff-editor` setting affects the default tool used for editing diffs
(e.g.  `jj split`, `jj squash -i`). If it is not set, the special value
`:builtin` is used. It launches a built-in TUI tool (known as [scm-diff-editor])
to edit the diff in your terminal.

[scm-diff-editor]: https://github.com/arxanas/scm-record?tab=readme-ov-file#scm-diff-editor

You can try a different tool temporarily by doing e.g. `jj split --tool meld` or
you can set the option to change the default. This requires that you have an
appropriate tool installed, e.g. [Meld](https://meldmerge.org/) to use the
`meld` diff editor.

**Suggestion:** If possible, it is recommended to try an external diff tool like
`meld` (see below for some other possibilities) for splitting commits and other
diff editing, in addition to the built-in diff editor. It is good to know the
capabilities of both. The built-in diff editor does not require external tools
to be available, is faster for tasks like picking hunks, and does not require
leaving the terminal. External tools give you the flexibility of picking out
portions of lines from the diff or even arbitrarily editing the text of the
files.

If `ui.diff-editor` is a string, e.g. `"meld"`, the arguments will be read from
the following config keys.

```toml
[merge-tools.meld]
# program = "meld"      # Defaults to the name of the tool if not specified
program = "/path/to/meld" # May be necessary if `meld` is not in the PATH
edit-args = ["--newtab", "$left", "$right"]
```

`jj` makes the following substitutions:

- `$left` and `$right` are replaced with the paths to the left and right
  directories to diff respectively.

- If no `edit-args` are specified, `["$left", "$right"]` are set by default.

Finally, `ui.diff-editor` can be a list that specifies a command and its arguments.

Some examples:

```toml
[ui]
# Use merge-tools.meld.edit-args
diff-editor = "meld"  # Or `kdiff3`, or `diffedit3`, ...
# Specify edit-args inline
diff-editor = ["/path/to/binary", "--be-helpful", "$left", "$right"]
# Equivalent to ["binary", "$left", "$right"] arguments by default
diff-editor = "binary"
```


### Experimental 3-pane diff editing

We offer two special "3-pane" diff editor configs:

- `meld-3`, which requires installing [Meld](https://meldmerge.org/), and
- `diffedit3`, which requires installing [`diffedit3`](https://github.com/ilyagr/diffedit3/releases).

`Meld` is a graphical application that is recommended, but can be difficult to
install in some situations. `diffedit3` is designed to be easy to install and to
be usable in environments where Meld is difficult to use (e.g. over SSH via port
forwarding). `diffedit3` starts a local server that can be accessed via a web
browser, similarly to [Jupyter](https://jupyter.org/).

There is also the `diffedit3-ssh` which is similar to `diffedit3` but does not
try to open the web browser pointing to the local server (the URL
printed to the terminal) automatically. `diffedit3-ssh` also always uses ports in between
17376-17380 and fails if they are all busy. This can be useful when working
over SSH. Open the fold below for more details of how to set that up.

<details>
<summary> Tips for using `diffedit3-ssh` over SSH </summary>

To use `diffedit3` over SSH, you need to set up port forwarding. One way to do
this is to start SSH as follows (copy-paste the relevant lines):

```shell
ssh -L 17376:localhost:17376 \
    -L 17377:localhost:17377 \
    -L 17378:localhost:17378 \
    -L 17379:localhost:17379 \
    -L 17380:localhost:17380 \
    myhost.example.com
```

`diffedit3-ssh` is set up to use these 5 ports by default. Usually, only the
first of them will be used. The rest are used if another program happens to use
one of them, or if you run multiple instances of `diffedit3` at the same time.

Another way is to add a snippet to `~/.ssh/config`:

```
Host myhost
    User     myself
    Hostname myhost.example.com
    LocalForward 17376 localhost:17376
    LocalForward 17377 localhost:17377
    LocalForward 17378 localhost:17378
    LocalForward 17379 localhost:17379
    LocalForward 17380 localhost:17380
```

With that configuration, you should be able to simply `ssh myhost`.

</details>


Setting either `ui.diff-editor = "meld-3"` or `ui.diff-editor = "diffedit3"`
will result in the diff editor showing 3 panes: the diff on the left and right,
and an editing pane in the middle. This allow you to see both sides of the
original diff while editing.

If you use `ui.diff-editor = "meld-3"`, note that you can still get the 2-pane
Meld view using `jj diff --tool meld`. `diffedit3` has a button you can use to
switch to a 2-pane view.

To configure other diff editors in this way, you can include `$output` together
with `$left` and `$right` in `merge-tools.TOOL.edit-args`. `jj` will replace
`$output` with the directory where the diff editor will be expected to put the
result of the user's edits. Initially, the contents of `$output` will be the
same as the contents of `$right`.

### `JJ-INSTRUCTIONS`

When editing a diff, jj will include a synthetic file called `JJ-INSTRUCTIONS`
in the diff with instructions on how to edit the diff. Any changes you make to
this file will be ignored. To suppress the creation of this file, set
`ui.diff-instructions = false`.

### Using Vim as a diff editor

Using `ui.diff-editor = "vimdiff"` is possible but not recommended. For a better
experience, you can follow [instructions from the Wiki] to configure the
[DirDiff Vim plugin] and/or the [vimtabdiff Python script].

[instructions from the Wiki]: https://github.com/jj-vcs/jj/wiki/Vim#using-vim-as-a-diff-tool

[DirDiff Vim plugin]: https://github.com/will133/vim-dirdiff
[vimtabdiff Python script]: https://github.com/balki/vimtabdiff

## 3-way merge tools for conflict resolution

The `ui.merge-editor` key specifies the tool used for three-way merge tools
by `jj resolve`. For example:

```toml
[ui]
# Use merge-tools.meld.merge-args
merge-editor = "meld"  # Or "vscode" or "vscodium" or "kdiff3" or "vimdiff"
# Specify merge-args inline
merge-editor = ["meld", "$left", "$base", "$right", "-o", "$output"]
```

The "vscode", "vscodium", "meld", "kdiff3", and "vimdiff" tools can be used out of the box,
as long as they are installed.

Using VS Code as a merge tool works well with VS Code's [Remote
Development](https://code.visualstudio.com/docs/remote/remote-overview)
functionality, as long as `jj` is called from VS Code's terminal.

### Setting up a custom merge tool

To use a different tool named `TOOL`, the arguments to pass to the tool MUST be
specified either inline or in the `merge-tools.TOOL.merge-args` key. As an
example of how to set this key and other tool configuration options, here is
the out-of-the-box configuration of the three default tools. (There is no need
to copy it to your config file verbatim, but you are welcome to customize it.)

```toml
[merge-tools.kdiff3]
# program  = "kdiff3"     # Defaults to the name of the tool if not specified
merge-args = ["$base", "$left", "$right", "-o", "$output", "--auto"]
[merge-tools.meld]
merge-args = ["$left", "$base", "$right", "-o", "$output", "--auto-merge"]

[merge-tools.vimdiff]
merge-args = ["-f", "-d", "$output", "-M",
    "$left", "$base", "$right",
    "-c", "wincmd J", "-c", "set modifiable",
    "-c", "set write"]
program = "vim"
merge-tool-edits-conflict-markers = true    # See below for an explanation
```

`jj` makes the following substitutions:

- `$output` (REQUIRED) is replaced with the name of the file that the merge tool
  should output. `jj` will read this file after the merge tool exits.

- `$left` and `$right` are replaced with the paths to two files containing the
  content of each side of the conflict.

- `$base` is replaced with the path to a file containing the contents of the
  conflicted file in the last common ancestor of the two sides of the conflict.

### Editing conflict markers with a tool or a text editor

By default, the merge tool starts with an empty output file. If the tool puts
anything into the output file and exits with the 0 exit code,
`jj` assumes that the conflict is fully resolved, while if the tool exits with
a non-zero exit code, `jj` assumes that the merge should be cancelled.
This is appropriate for most graphical merge tools.

For merge tools which try to automatically resolve conflicts without user input,
this behavior may not be desired. For instance, some automatic merge tools use
an exit code of 1 to indicate that some conflicts were unable to be resolved and
that the output file should contain conflict markers. In that case, you could
set the config option `merge-tools.TOOL.merge-conflict-exit-codes = [1]` to tell
`jj` to expect conflict markers in the output file if the exit code is 1. If a
merge tool produces output using Git's "diff3" conflict style, `jj` should be
able to parse it correctly, so many Git merge drivers should be usable with `jj`
as well.

Some tools (e.g. `vimdiff`) can present a multi-way diff but don't resolve
conflict themselves. When using such tools, `jj`
can help you by populating the output file with conflict markers before starting
the merge tool (instead of leaving the output file empty and letting the merge
tool fill it in). To do that, set the
`merge-tools.vimdiff.merge-tool-edits-conflict-markers = true` option.

With this option set, if the output file still contains conflict markers after
the conflict is done, `jj` assumes that the conflict was only partially resolved
and parses the conflict markers to get the new state of the conflict. The
conflict is considered fully resolved when there are no conflict markers left.
The conflict marker style can also be customized per tool using the
`merge-tools.TOOL.conflict-marker-style` option, which takes the same values as
[`ui.conflict-marker-style`](#conflict-marker-style).

## Code formatting and other file content transformations

The `jj fix` command allows you to efficiently rewrite files in complex commit
graphs with no risk of introducing conflicts, using tools like `clang-format` or
`prettier`. The tools run as subprocesses that take file content on standard
input and repeat it, with any desired changes, on standard output. The file is
only rewritten if the subprocess produces a successful exit code.

### Enforce coding style rules

Suppose you want to use `clang-format` to format your `*.c` and `*.h` files,
as well as sorting their `#include` directives.

`jj fix` provides the file content anonymously on standard input, but the name
of the file being formatted may be important for include sorting or other output
like error messages. To address this, you can use the `$path` substitution to
provide the name of the file in a command argument.

```toml
[fix.tools.clang-format]
command = ["/usr/bin/clang-format", "--sort-includes", "--assume-filename=$path"]
patterns = ["glob:'**/*.c'",
            "glob:'**/*.h'"]
```

### Sort and remove duplicate lines from a file

`jj fix` can also be used with tools that are not considered code formatters.

Suppose you have a list of words in a text file in your repository, and you want
to keep the file sorted alphabetically and remove any duplicate words.

```toml
[fix.tools.sort-word-list]
command = ["sort", "-u"]
patterns = ["word_list.txt"]
```

### Execution order of tools

If two or more tools affect the same file, they are executed in the ascending
lexicographical order of their configured names. This will remain as a tie
breaker if other ordering mechanisms are introduced in the future. If you use
numbers in tool names to control execution order, remember to include enough
leading zeros so that, for example, `09` sorts before `10`.

Suppose you want to keep only the 10 smallest numbers in a text file that
contains one number on each line. This can be accomplished with `sort` and
`head`, but execution order is important.

```toml
[fix.tools.1-sort-numbers-file]
command = ["sort", "-n"]
patterns = ["numbers.txt"]

[fix.tools.2-truncate-numbers-file]
command = ["head", "-n", "10"]
patterns = ["numbers.txt"]
```

## Commit Signing

`jj` can be configured to sign and verify the commits it creates using either
GnuPG or SSH signing keys.

To do this you need to configure a signing backend.

Setting the backend to `"none"` disables signing.

### GnuPG Signing

```toml
[signing]
sign-all = true
backend = "gpg"
key = "4ED556E9729E000F"
## You can set `key` to anything accepted by `gpg -u`
# key = "signing@example.com"
```

By default the gpg backend will look for a `gpg` binary on your path. If you want
to change the program used or specify a path to `gpg` explicitly you can set:

```toml
[signing]
backends.gpg.program = "gpg2"
```

Also by default the gpg backend will ignore key expiry when verifying commit signatures.
To consider expired keys as invalid you can set:

```toml
[signing]
backends.gpg.allow-expired-keys = false
```

### SSH Signing

```toml
[signing]
sign-all = true
backend = "ssh"
key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGj+J6N6SO+4P8dOZqfR1oiay2yxhhHnagH52avUqw5h"
## You can also use a path instead of embedding the key
# key = "~/.ssh/id_for_signing.pub"
```

By default the ssh backend will look for a `ssh-keygen` binary on your path. If you want
to change the program used or specify a path to `ssh-keygen` explicitly you can set:

```toml
[signing]
backends.ssh.program = "/path/to/ssh-keygen"
```

When verifying commit signatures the ssh backend needs to be provided with an allowed-signers
file containing the public keys of authors whose signatures you want to be able to verify.

You can find the format for this file in the
[ssh-keygen man page](https://man.openbsd.org/ssh-keygen#ALLOWED_SIGNERS). This can be provided
as follows:

```toml
[signing]
backends.ssh.allowed-signers = "/path/to/allowed-signers"
```

## Git settings

### Default remotes for `jj git fetch` and `jj git push`

By default, if a single remote exists it is used for `jj git fetch` and `jj git
push`; however if multiple remotes exist, the default remote is assumed to be
named `"origin"`, just like in Git. Sometimes this is undesirable, e.g. when you
want to fetch from a different remote than you push to, such as a GitHub fork.

To change this behavior, you can modify the [repository
configuration](#config-files-and-toml) variable `git.fetch`, which can be a
single remote, or a list of remotes to fetch from multiple places:

```sh
jj config set --repo git.fetch "upstream"
jj config set --repo git.fetch '["origin", "upstream"]'
```

Similarly, you can also set the variable `git.push` to cause `jj git push` to
push to a different remote:

```sh
jj config set --repo git.push "github"
```

Note that unlike `git.fetch`, `git.push` can currently only be a single remote.
This is not a hard limitation, and could be changed in the future if there is
demand.

### Automatic local bookmark creation

When `jj` imports a new remote-tracking bookmark from Git, it can also create a
local bookmark with the same name. This feature is disabled by default because it
may be undesirable in some repositories, e.g.:

- There is a remote with a lot of historical bookmarks that you don't
  want to be exported to the co-located Git repo.
- There are multiple remotes with conflicting views of that bookmark,
  resulting in an unhelpful conflicted state.

You can enable this behavior by setting `git.auto-local-bookmark` like so,

```toml
[git]
auto-local-bookmark = true
```

This setting is applied only to new remote bookmarks. Existing remote bookmarks
can be tracked individually by using `jj bookmark track`/`untrack` commands.

```shell
# import feature1 bookmark and start tracking it
jj bookmark track feature1@origin
# delete local gh-pages bookmark and stop tracking it
jj bookmark delete gh-pages
jj bookmark untrack gh-pages@upstream
```

### Abandon commits that became unreachable in Git

By default, when `jj` imports refs from Git, it will look for commits that used
to be [reachable][reachable] but no longer are reachable. Those commits will
then be abandoned, and any descendant commits will be rebased off of them (as
usual when commits are abandoned). You can disable this behavior and instead
leave the Git-unreachable commits in your repo by setting:

```toml
[git]
abandon-unreachable-commits = false
```

[reachable]: https://git-scm.com/docs/gitglossary/#Documentation/gitglossary.txt-aiddefreachableareachable

### Prefix for generated bookmarks on push

`jj git push --change` generates bookmark names with a prefix of "push-" by
default. You can pick a different prefix by setting `git.push-bookmark-prefix`. For
example:

```toml
[git]
push-bookmark-prefix = "martinvonz/push-"
```

### Set of private commits

You can configure the set of private commits by setting `git.private-commits` to
a revset. The value is a revset of commits that Jujutsu will refuse to push. If
unset, all commits are eligible to be pushed.

```toml
[git]
# Prevent pushing work in progress or anything explicitly labeled "private"
private-commits = "description(glob:'wip:*') | description(glob:'private:*')"
```

If a commit is in `git.private-commits` but is already on the remote, then it is
not considered a private commit. Commits that are immutable are also excluded
from the private set.

Private commits prevent their descendants from being pushed, since doing so
would require pushing the private commit as well.

## Filesystem monitor

In large repositories, it may be beneficial to use a "filesystem monitor" to
track changes to the working copy. This allows `jj` to take working copy
snapshots without having to rescan the entire working copy.

This is governed by the `core.fsmonitor` option. Currently, the valid values are
`"none"` or `"watchman"`.

### Watchman

To configure the Watchman filesystem monitor, set
`core.fsmonitor = "watchman"`. Ensure that you have [installed the Watchman
executable on your system](https://facebook.github.io/watchman/docs/install).

You can configure `jj` to use watchman triggers to automatically create
snapshots on filesystem changes by setting
`core.watchman.register_snapshot_trigger = true`.

You can check whether Watchman is enabled and whether it is installed correctly
using `jj debug watchman status`.

## Snapshot settings

### Paths to automatically track

All new files in the working copy that don't match the ignore patterns are
tracked by default. You can set the `snapshot.auto-track` to set which paths
get automatically tracked when they're added to the working copy. See the
[fileset documentation](filesets.md) for the syntax. Files with paths matching
[ignore files](working-copy.md#ignored-files) are never tracked automatically.

You can use `jj file untrack` to untrack a file while keeping it in the working
copy. However, first [ignore](working-copy.md#ignored-files) them or remove them
from the `snapshot.auto-track` patterns; otherwise they will be immediately
tracked again.

### Maximum size for new files

By default, as an anti-footgun measure, `jj` will refuse to add new files to the
snapshot that are larger than a certain size; the default is 1MiB. This can be
changed by setting `snapshot.max-new-file-size` to a different value. For
example:

```toml
[snapshot]
max-new-file-size = "10MiB"
# the following is equivalent
max-new-file-size = 10485760
```

The value can be specified using a human readable string with typical suffixes;
`B`, `MiB`, `GB`, etc. By default, if no suffix is provided, or the value is a
raw integer literal, the value is interpreted as if it were specified in bytes.

Files that already exist in the working copy are not subject to this limit.

Setting this value to zero will disable the limit entirely.

## Ways to specify `jj` config: details

### User config file

An easy way to find the user config file is:

```bash
jj config path --user
```

The rest of this section covers the details of where this file can be located.

On all platforms, the user's global `jj` configuration file is located at either
`~/.jjconfig.toml` (where `~` represents `$HOME` on Unix-likes, or
`%USERPROFILE%` on Windows) or in a platform-specific directory. The
platform-specific location is recommended for better integration with platform
services. It is an error for both of these files to exist.

| Platform | Value                                              | Example                                                   |
| :------- | :------------------------------------------------- | :-------------------------------------------------------- |
| Linux    | `$XDG_CONFIG_HOME/jj/config.toml`                  | `/home/alice/.config/jj/config.toml`                      |
| macOS    | `$HOME/Library/Application Support/jj/config.toml` | `/Users/Alice/Library/Application Support/jj/config.toml` |
| Windows  | `{FOLDERID_RoamingAppData}\jj\config.toml`         | `C:\Users\Alice\AppData\Roaming\jj\config.toml`           |

The location of the `jj` config file can also be overridden with the
`JJ_CONFIG` environment variable. If it is not empty, it should contain the path
to a TOML file that will be used instead of any configuration file in the
default locations. For example,

```shell
env JJ_CONFIG=/dev/null jj log       # Ignores any settings specified in the config file.
```

### Specifying config on the command-line

You can use one or more `--config`/`--config-file` options on the command line
to specify additional configuration settings. This overrides settings defined in
config files or environment variables. For example,

```shell
jj --config=ui.color=always --config=ui.diff-editor=kdiff3 split
```

Config value should be specified as a TOML expression. If string value doesn't
contain any TOML constructs (such as array notation), quotes can be omitted.

To load an entire TOML document, use `--config-file`:

```shell
jj --config-file=extra-config.toml log
```

### Conditional variables

You can conditionally enable config variables by using `--when` and
`[[--scope]]` tables. Variables defined in `[[--scope]]` tables are expanded to
the root table. `--when` specifies the condition to enable the scope table.

```toml
[user]
name = "YOUR NAME"
email = "YOUR_DEFAULT_EMAIL@example.com"

# override user.email if the repository is located under ~/oss
[[--scope]]
--when.repositories = ["~/oss"]
[--scope.user]
email = "YOUR_OSS_EMAIL@example.org"
```

Condition keys:

* `--when.repositories`: List of paths to match the repository path prefix.

If no conditions are specified, table is always enabled.
