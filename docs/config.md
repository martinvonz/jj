# Configuration

These are the config settings available to jj/Jujutsu.


## Config files and TOML

The config settings are loaded from the following locations. Less common ways to
specify `jj` config settings are discussed in a later section.

* `~/.jjconfig.toml` (global)
* `.jj/repo/config.toml` (per-repository)

See the [TOML site] and the [syntax guide] for a description of the syntax.

[TOML site]: https://toml.io/en/
[syntax guide]: https://toml.io/en/v1.0.0

The first thing to remember is that the value of a setting (the part to the
right of the `=` sign) should be surrounded in quotes if it's a string.

### Dotted style and headings
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
colors."commit_id prefix".bold = true

# is equivalent to:
[template-aliases]
"format_short_id(id)" = "id.shortest(12)"

[colors]
"commit_id prefix" = { bold = true }
```

Jujutsu favors the dotted style in these instructions, if only because it's
easier to write down in an unconfusing way. If you are confident with TOML
then use whichever suits you in your config. If you mix dotted keys and headings,
**put the dotted keys before the first heading**.

That's probably enough TOML to keep you out of trouble but the [syntax guide] is
very short if you ever need to check.


## User settings

```toml
user.name = "YOUR NAME"
user.email = "YOUR_EMAIL@example.com"
```

Don't forget to change these to your own details!

## UI settings

### Colorizing output

Possible values are `always`, `never` and `auto` (default: `auto`).
`auto` will use color only when writing to a terminal.

This setting overrides the `NO_COLOR` environment variable (if set).

```toml
ui.color = "never" # Turn off color
```

### Custom colors and styles

You can customize the colors used for various elements of the UI. For example:

```toml
colors.commit_id = "green"
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

If you use a string value for a color, as in the example above, it will be used
for the foreground color. You can also set the background color, or make the
text bold or underlined. For that, you need to use a table:

```toml
colors.commit_id = { fg = "green", bg = "red", bold = true, underline = true }
```

The key names are called "labels". The above used `commit_id` as label. You can
also create rules combining multiple labels. The rules work a bit like CSS
selectors. For example, if you want to color commit IDs green in general but
make the commit ID of the working-copy commit also be underlined, you can do
this:

```toml
colors.commit_id = "green"
colors."working_copy commit_id" = { underline = true }
```

Parts of the style that are not overridden - such as the foreground color in the
example above - are inherited from the parent style.

Which elements can be colored is not yet documented, but see
the [default color configuration](https://github.com/martinvonz/jj/blob/main/src/config/colors.toml)
for some examples of what's possible.

### Default command

When `jj` is run with no explicit subcommand, the value of the
`ui.default-command` setting will used instead. Possible values are any valid
subcommand name, subcommand alias, or user-defined alias (defaults to `"log"`).

```toml
ui.default-command = "log"
```

### Diff format

```toml
# Possible values: "color-words" (default), "git", "summary"
ui.diff.format = "git"
```

### Graph style

```toml
# Possible values: "curved" (default), "square", "ascii", "ascii-large",
# "legacy" 
ui.graph.style = "square"
```

### Wrap log content

If enabled, `log`/`obslog`/`op log` content will be wrapped based on 
the terminal width.

```toml
ui.log-word-wrap = true
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

### Relative timestamps

Can be customized by the `format_timestamp()` template alias.

```toml
[template-aliases]
# Full timestamp in ISO 8601 format (default)
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

## Pager

The default pager is can be set via `ui.pager` or the `PAGER` environment
variable. The priority is as follows (environment variables are marked with
a `$`):

`ui.pager` > `$PAGER`

`less -FRX` is the default pager in the absence of any other setting.

### Processing contents to be paged

If you'd like to pass the output through a formatter e.g.
[`diff-so-fancy`](https://github.com/so-fancy/diff-so-fancy) before piping it
through a pager you must do it using a subshell as, unlike `git` or `hg`, the
command will be executed directly. For example:

`ui.pager = ["sh", "-c", "diff-so-fancy | less -RFX"]`

## Aliases

You can define aliases for commands, including their arguments. For example:

```toml
# `jj l` shows commits on the working-copy commit's (anonymous) branch
# compared to the `main` branch
aliases.l = ["log", "-r", "(main..@): | (main..@)-"]
```

## Editor

The default editor is set via `ui.editor`, though there are several places to
set it. The priority is as follows (environment variables are marked with
a `$`):

`$JJ_EDITOR` > `ui.editor` > `$VISUAL` > `$EDITOR`

Pico is the default editor in the absence of any other setting, but you could
set it explicitly too.

```toml
ui.editor = "pico"
```

To use NeoVim instead:

```toml
ui.editor = "nvim"
```

For GUI editors you possibly need to use a `-w` or `--wait`. Some examples:

```toml
ui.editor = "code -w"       # VS Code
ui.editor = "bbedit -w"     # BBEdit
ui.editor = "subl -n -w"    # Sublime Text
ui.editor = "mate -w"       # TextMate
ui.editor = ["C:/Program Files/Notepad++/notepad++.exe",
    "-multiInst", "-notabbar", "-nosession", "-noPlugin"] # Notepad++
ui.editor = "idea --temp-project --wait"   #IntelliJ
```

Obviously, you would only set one line, don't copy them all in!

## Editing diffs

The `ui.diff-editor` setting affects the tool used for editing diffs (e.g.
`jj split`, `jj amend -i`). The default is `meld`.

`jj` makes the following substitutions:

- `$left` and `$right` are replaced with the paths to the left and right
  directories to diff respectively.

If no arguments are specified, `["$left", "$right"]` are set by default.

For example:

```toml
# Use merge-tools.kdiff3.edit-args
ui.diff-editor = "kdiff3"
# Specify edit-args inline
ui.diff-editor = ["kdiff3", "--merge", "$left", "$right"]
```

If `ui.diff-editor` consists of a single word, e.g. `"kdiff3"`, the arguments
will be read from the following config keys.

```toml
# merge-tools.kdiff3.program = "kdiff3"      # Defaults to the name of the tool if not specified
merge-tools.kdiff3.edit-args = [
    "--merge", "--cs", "CreateBakFiles=0", "$left", "$right"]
```

### Using Vim as a diff editor

Using `ui.diff-editor = "vimdiff"` is possible but not recommended. For a better
experience, you can follow these [instructions] to configure
the [`DirDiff` Vim plugin].

[instructions]: https://gist.github.com/ilyagr/5d6339fb7dac5e7ab06fe1561ec62d45

[`DirDiff` Vim plugin]: https://github.com/will133/vim-dirdiff

## 3-way merge tools for conflict resolution

The `ui.merge-editor` key specifies the tool used for three-way merge tools
by `jj resolve`. For example:

```toml
# Use merge-tools.meld.merge-args
ui.merge-editor = "meld"  # Or "kdiff3" or "vimdiff"
# Specify merge-args inline
ui.merge-editor = ["meld", "$left", "$base", "$right", "-o", "$output"]
```

The "meld", "kdiff3", and "vimdiff" tools can be used out of the box, as long as
they are installed.

To use a different tool named `TOOL`, the arguments to pass to the tool MUST be
specified either inline or in the `merge-tools.TOOL.merge-args` key. As an
example of how to set this key and other tool configuration options, here is
the out-of-the-box configuration of the three default tools. (There is no need
to copy it to your config file verbatim, but you are welcome to customize it.)

```toml
# merge-tools.kdiff3.program  = "kdiff3"     # Defaults to the name of the tool if not specified
merge-tools.kdiff3.merge-args = ["$base", "$left", "$right", "-o", "$output", "--auto"]
merge-tools.meld.merge-args = ["$left", "$base", "$right", "-o", "$output", "--auto-merge"]

merge-tools.vimdiff.merge-args = ["-f", "-d", "$output", "-M",
    "$left", "$base", "$right",
    "-c", "wincmd J", "-c", "set modifiable",
    "-c", "set write"]
merge-tools.vimdiff.program = "vim"
merge-tools.vimdiff.merge-tool-edits-conflict-markers = true    # See below for an explanation
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
anything into the output file, and exits with the 0 exit code,
`jj` assumes that the conflict is fully resolved. This is appropriate for most
graphical merge tools.

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

## Git settings

### Automatic local branch creation

By default, when `jj` imports a remote-tracking branch from Git, it also
creates a local branch with the same name. In some repositories, this
may be undesirable, e.g.:

- There is a remote with a lot of historical branches that you don't
  want to be exported to the co-located Git repo.
- There are multiple remotes with conflicting views of that branch,
  resulting in an unhelpful conflicted state.

You can disable this behavior by setting `git.auto-local-branch` like
so,

    git.auto-local-branch = false

Note that this setting may make it easier to accidentally delete remote
branches. Since the local branch isn't created, the remote branch will be
deleted if you push the branch with `jj git push --branch` or `jj git push
--all`.

# Alternative ways to specify configuration settings

Instead of `~/.jjconfig.toml`, the config settings can be located under
a platform-specific directory. It is an error for both of these files to exist.

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

You can use one or more `--config-toml` options on the command line to specify
additional configuration settings. This overrides settings defined in config
files or environment variables. For example,

```shell
jj --config-toml='ui.color="always"' --config-toml='ui.difftool="kdiff3"' split
```

Config specified this way must be valid TOML. In particular, string values must
be surrounded by quotes. To pass these quotes to `jj`, most shells require
surrounding those quotes with single quotes as shown above.

In `sh`-compatible shells, `--config-toml` can be used to merge entire TOML
files with the config specified in `.jjconfig.toml`:

```shell
jj --config-toml="$(cat extra-config.toml)" log
```
