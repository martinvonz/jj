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
in
the list are overridden by the settings from later items if they disagree. Every
type of config except for the built-in settings is optional.

See the [TOML site] and the [syntax guide] for a detailed description of the
syntax. We cover some of the basics below.

[the user config file]: #user-config-file
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

You can also use a 6-digit hex code for more control over the exact color used:

```toml
colors.change_id = "#ff1525"
```

If you use a string value for a color, as in the examples above, it will be used
for the foreground color. You can also set the background color, or make the
text bold or underlined. For that, you need to use a table:

```toml
colors.commit_id = { fg = "green", bg = "#ff1525", bold = true, underline = true }
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
the [default color configuration](https://github.com/martinvonz/jj/blob/main/cli/src/config/colors.toml)
for some examples of what's possible.

### Default command

When `jj` is run with no explicit subcommand, the value of the
`ui.default-command` setting will be used instead. Possible values are any valid
subcommand name, subcommand alias, or user-defined alias (defaults to `"log"`).

```toml
ui.default-command = "log"
```

### Default description

The value of the `ui.default-description` setting will be used to prepopulate
the editor when describing changes with an empty description. This could be a
useful reminder to fill in things like BUG=, TESTED= etc.

```toml
ui.default-description = "\n\nTESTED=TODO"
```

### Diff format

```toml
# Possible values: "color-words" (default), "git", "summary"
ui.diff.format = "git"
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

### Set of immutable commits

You can configure the set of immutable commits via `revset-aliases."immutable_heads()"`.
The default set of immutable heads is `trunk() | tags()`. For example, to
prevent rewriting commits on `main@origin` and commits authored by other
users:

```toml
# The `main.. &` bit is an optimization to scan for non-`mine()` commits only
# among commits that are not in `main`.
revset-aliases."immutable_heads()" = "main@origin | (main@origin.. & ~mine())"
```

Ancestors of the configured set are also immutable. The root commit is always
immutable even if the set is empty.

### Default revisions to log

You can configure the revisions `jj log` without `-r` should show.

```toml
# Show commits that are not in `main@origin`
revsets.log = "main@origin.."
```

### Graph style

```toml
# Possible values: "curved" (default), "square", "ascii", "ascii-large"
ui.graph.style = "square"
```

#### Node style

The symbols used to represent commits or operations can be customized via
templates.

  * `templates.log_node` for commits (with `Option<Commit>` keywords)
  * `templates.op_log_node` for operations (with `Operation` keywords)

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

To get shorter prefixes for certain revisions, set `revsets.short-prefixes`:

```toml
# Prioritize the current branch
revsets.short-prefixes = "(main..@)::"
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

## Pager

The default pager is can be set via `ui.pager` or the `PAGER` environment
variable. The priority is as follows (environment variables are marked with
a `$`):

`ui.pager` > `$PAGER`

`less -FRX` is the default pager in the absence of any other setting, except
on Windows where it is `:builtin`.

The special value `:builtin` enables usage of the
[integrated pager](https://github.com/arijit79/minus/). It is likely if you
are using a standard Linux distro, your system has `$PAGER` set already
and that will be preferred over the built-in. To use the built-in:

```
jj config set --user ui.pager :builtin
```

It is possible the default will change to `:builtin` for all platforms in the
future.

Additionally, paging behavior can be toggled via `ui.paginate` like so:

```toml
# Enable pagination for commands that support it (default)
ui.paginate = "auto"
# Disable all pagination, equivalent to using --no-pager
ui.paginate = "never"
```

### Processing contents to be paged

If you'd like to pass the output through a formatter e.g.
[`diff-so-fancy`](https://github.com/so-fancy/diff-so-fancy) before piping it
through a pager you must do it using a subshell as, unlike `git` or `hg`, the
command will be executed directly. For example:

```toml
ui.pager = ["sh", "-c", "diff-so-fancy | less -RFX"]
```

## Aliases

You can define aliases for commands, including their arguments. For example:

```toml
# `jj l` shows commits on the working-copy commit's (anonymous) branch
# compared to the `main` branch
aliases.l = ["log", "-r", "(main..@):: | (main..@)-"]
```

## Editor

The default editor is set via `ui.editor`, though there are several places to
set it. The priority is as follows (environment variables are marked with
a `$`):

`$JJ_EDITOR` > `ui.editor` > `$VISUAL` > `$EDITOR`

Pico is the default editor (Notepad on Windows) in the absence of any other
setting, but you could set it explicitly too.

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

The `ui.diff-editor` setting affects the tool used for editing diffs (e.g.  `jj
split`, `jj squash -i`). The default is the special value `:builtin`, which
launches a built-in TUI tool (known as [scm-diff-editor]) to edit the diff in
your terminal.

[scm-diff-editor]: https://github.com/arxanas/scm-record?tab=readme-ov-file#scm-diff-editor

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

### Experimental 3-pane diff editing

The special `"meld-3"` diff editor sets up Meld to show 3 panes: the sides of
the diff on the left and right, and an editing pane in the middle. This allow
you to see both sides of the original diff while editing. If you use
`ui.diff-editor = "meld-3"`, note that you can still get the 2-pane Meld view
using `jj diff --tool meld`.

To configure other diff editors, you can include `$output` together with `$left`
and `$right` in `merge-tools.TOOL.edit-args`. `jj` will replace `$output` with
the directory where the diff editor will be expected to put the result of the
user's edits. Initially, the contents of `$output` will be the same as the
contents of `$right`.

### `JJ-INSTRUCTIONS`

When editing a diff, jj will include a synthetic file called `JJ-INSTRUCTIONS`
in the diff with instructions on how to edit the diff. Any changes you make to
this file will be ignored. To suppress the creation of this file, set
`ui.diff-instructions = false`.

### Using Vim as a diff editor

Using `ui.diff-editor = "vimdiff"` is possible but not recommended. For a better
experience, you can follow [instructions from the Wiki] to configure the
[DirDiff Vim plugin] and/or the [vimtabdiff Python script].

[instructions from the Wiki]: https://github.com/martinvonz/jj/wiki/Vim#using-vim-as-a-diff-tool

[DirDiff Vim plugin]: https://github.com/will133/vim-dirdiff
[vimtabdiff Python script]: https://github.com/balki/vimtabdiff

## 3-way merge tools for conflict resolution

The `ui.merge-editor` key specifies the tool used for three-way merge tools
by `jj resolve`. For example:

```toml
# Use merge-tools.meld.merge-args
ui.merge-editor = "meld"  # Or "vscode" or "kdiff3" or "vimdiff"
# Specify merge-args inline
ui.merge-editor = ["meld", "$left", "$base", "$right", "-o", "$output"]
```

The "vscode", "meld", "kdiff3", and "vimdiff" tools can be used out of the box,
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

## Commit Signing

`jj` can be configured to sign and verify the commits it creates using either 
GnuPG or SSH signing keys.

To do this you need to configure a signing backend.

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
signing.backends.gpg.program = "gpg2"
```

Also by default the gpg backend will ignore key expiry when verifying commit signatures.
To consider expired keys as invalid you can set:

```toml
signing.backends.gpg.allow-expired-keys = false
```

### SSH Signing

```toml
[signing]
sign-all = true
backend = "ssh"
key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGj+J6N6SO+4P8dOZqfR1oiay2yxhhHnagH52avUqw5h"
## You can also use a path instead of embedding the key
# key = "/home/me/.ssh/id_for_signing.pub"
```

By default the ssh backend will look for a `ssh-keygen` binary on your path. If you want
to change the program used or specify a path to `ssh-keygen` explicitly you can set:

```toml
signing.backends.ssh.program = "/path/to/ssh-keygen"
```

When verifying commit signatures the ssh backend needs to be provided with an allowed-signers
file containing the public keys of authors whose signatures you want to be able to verify.

You can find the format for this file in the 
[ssh-keygen man page](https://man.openbsd.org/ssh-keygen#ALLOWED_SIGNERS). This can be provided 
as follows:

```toml
signing.backends.ssh.allowed-signers = "/path/to/allowed-signers"
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

### Automatic local branch creation

When `jj` imports a new remote-tracking branch from Git, it can also create a
local branch with the same name. This feature is disabled by default because it
may be undesirable in some repositories, e.g.:

- There is a remote with a lot of historical branches that you don't
  want to be exported to the co-located Git repo.
- There are multiple remotes with conflicting views of that branch,
  resulting in an unhelpful conflicted state.

You can enable this behavior by setting `git.auto-local-branch` like so,

```toml
git.auto-local-branch = true
```

This setting is applied only to new remote branches. Existing remote branches
can be tracked individually by using `jj branch track`/`untrack` commands.

```shell
# import feature1 branch and start tracking it
jj branch track feature1@origin
# delete local gh-pages branch and stop tracking it
jj branch delete gh-pages
jj branch untrack gh-pages@upstream
```

### Abandon commits that became unreachable in Git

By default, when `jj` imports refs from Git, it will look for commits that used
to be [reachable][reachable] but no longer are reachable. Those commits will
then be abandoned, and any descendant commits will be rebased off of them (as
usual when commits are abandoned). You can disable this behavior and instead
leave the Git-unreachable commits in your repo by setting:

```toml
git.abandon-unreachable-commits = false
```

[reachable]: https://git-scm.com/docs/gitglossary/#Documentation/gitglossary.txt-aiddefreachableareachable

### Prefix for generated branches on push

`jj git push --change` generates branch names with a prefix of "push-" by
default. You can pick a different prefix by setting `git.push-branch-prefix`. For
example:

    git.push-branch-prefix = "martinvonz/push-"

## Filesystem monitor

In large repositories, it may be beneficial to use a "filesystem monitor" to
track changes to the working copy. This allows `jj` to take working copy
snapshots without having to rescan the entire working copy.

### Watchman

To configure the Watchman filesystem monitor, set
`core.fsmonitor = "watchman"`. Ensure that you have [installed the Watchman
executable on your system](https://facebook.github.io/watchman/docs/install).

Debugging commands are available under `jj debug watchman`.

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

You can use one or more `--config-toml` options on the command line to specify
additional configuration settings. This overrides settings defined in config
files or environment variables. For example,

```shell
jj --config-toml='ui.color="always"' --config-toml='ui.diff-editor="kdiff3"' split
```

Config specified this way must be valid TOML. In particular, string values must
be surrounded by quotes. To pass these quotes to `jj`, most shells require
surrounding those quotes with single quotes as shown above.

In `sh`-compatible shells, `--config-toml` can be used to merge entire TOML
files with the config specified in `.jjconfig.toml`:

```shell
jj --config-toml="$(cat extra-config.toml)" log
```
