# Configuration

These are the config settings available to jj/Jujutsu.

The config settings are loaded from the following locations. Less common ways
to specify `jj` config settings are discussed in a later section.

* `~/.jjconfig.toml` (global)
* `.jj/repo/config.toml` (per-repository)

See the [TOML site](https://toml.io/en/) for more on syntax.
One thing to remember is that anything under a heading can be dotted
e.g. `user.name = "YOUR NAME"` is equivalent to:

    [user]
      name = "YOUR NAME"

Headings only need to be set once in the real config file but Jujutsu
favors the dotted style in these instructions, if only because it's
easier to write down in an unconfusing way. If you are confident with
TOML then use whichever suits you in your config. If you mix the styles,
put the dotted keys before the first heading.

The other thing to remember is that the value of a setting (the part
to the right of the `=` sign) should be surrounded in quotes if it's
a string.  That's probably enough TOML to keep you out of trouble but
the syntax guide is very short if you ever need to check.


## User settings

    user.name = "YOUR NAME" 
    user.email = "YOUR_EMAIL@example.com"

Don't forget to change these to your own details!


## UI settings

### Colorizing output

Possible values are `always`, `never` and `auto` (default: `auto`). 
`auto` will use color only when writing to a terminal. 

This setting overrides the `NO_COLOR` environment variable (if set).

    ui.color = "never" # Turn off color

### Shortest unique prefixes for ids

    ui.unique-prefixes = "none"

Whether to highlight a unique prefix for commit & change ids. Possible
values are `brackets` and `none` (default: `brackets`).

### Relative timestamps

    ui.relative-timestamps = true

False by default, but setting to true will change timestamps to be rendered
as `x days/hours/seconds ago` instead of being rendered as a full timestamp.


## Pager

The default pager is can be set via `ui.pager` or the `PAGER` environment
variable.
The priority is as follows (environment variables are marked with a `$`):

`ui.pager` > `$PAGER`

`less -FRX` is the default pager in the absence of any other setting.


## Editor

The default editor is set via `ui.editor`, though there are several
places to set it.  The priority is as follows (environment variables
are marked with a `$`):

`$JJ_EDITOR` > `ui.editor` > `$VISUAL` > `$EDITOR`

Pico is the default editor in the absence of any other setting but you
could set it explicitly too.

    ui.editor = "pico"

To use NeoVim instead:

    ui.editor = "nvim"

For GUI editors you possibly need to use a `-w` or `--wait`. Some examples:

    ui.editor = "code -w"       # VS Code
    ui.editor = "bbedit -w"     # BBEdit
    ui.editor = "subl -n -w"    # Sublime Text
    ui.editor = "mate -w"       # TextMate
    ui.editor = ["C:/Program Files/Notepad++/notepad++.exe",
                 "-multiInst", "-notabbar", "-nosession", "-noPlugin"] # Notepad++
    ui.editor = "idea --temp-project --wait"   #IntelliJ

Obviously, you would only set one line, don't copy them all in!


## Editing diffs

The `ui.diff-editor` setting affects the tool used for editing diffs (e.g.
`jj split`, `jj amend -i`).  The default is `meld`. The left and right
directories to diff are passed as the first and second argument respectively.

For example:

    ui.diff-editor = "kdiff3"

Custom arguments can be added, and will be inserted before the paths
to diff:

    # merge-tools.kdiff3.program = "kdiff3"      # Defaults to the name of the tool if not specified
    merge-tools.kdiff3.edit-args = ["--merge", "--cs", "CreateBakFiles=0"]

### Using Vim as a diff editor

Using `ui.diff-editor = "vimdiff"` is possible but not recommended.
For a better experience, you can follow these [instructions] to
configure the [`DirDiff` Vim plugin].

[instructions]: https://gist.github.com/ilyagr/5d6339fb7dac5e7ab06fe1561ec62d45
[`DirDiff` Vim plugin]: https://github.com/will133/vim-dirdiff

## 3-way merge tools for conflict resolution

The `ui.merge-editor` key specifies the tool used for three-way merge
tools by `jj resolve`.  For example:

    ui.merge-editor = "meld"  # Or "kdiff3" or "vimdiff"

The "meld", "kdiff3", and "vimdiff" tools can be used out of the box,
as long as they are installed.

To use a different tool named `TOOL`, the arguments to pass to the tool
MUST be specified in the `merge-tools.TOOL.merge-args` key. As an example
of how to set this key and other tool configuration options, here is
the out-of-the-box configuration of the three default tools. (There is
no need to copy it to your config file verbatim, but you are welcome to
customize it.)

    # merge-tools.kdiff3.program  = "kdiff3"     # Defaults to the name of the tool if not specified
    merge-tools.kdiff3.merge-args = ["$base", "$left", "$right", "-o", "$output", "--auto"]
    merge-tools.meld.merge-args   = ["$left", "$base", "$right", "-o", "$output", "--auto-merge"]

    merge-tools.vimdiff.merge-args = ["-f", "-d", "$output", "-M",
                                      "$left", "$base", "$right",
                                      "-c", "wincmd J", "-c", "set modifiable",
                                      "-c", "set write"]
    merge-tools.vimdiff.program = "vim"
    merge-tools.vimdiff.merge-tool-edits-conflict-markers = true    # See below for an explanation

`jj` replaces the following arguments with the appropriate file names:

- `$output` (REQUIRED) is replaced with the name of the file that the
merge tool should output. `jj` will read this file after the merge tool
exits.

- `$left` and `$right` are replaced with the paths to two files containing
the content of each side of the conflict.

- `$base` is replaced with the path to a file containing the
contents of the conflicted file in the last common ancestor of the two
sides of the conflict.

### Editing conflict markers with a tool or a text editor

By default, the merge tool starts with an empty output file. If the tool
puts anything into the output file, and exits with the 0 exit code,
`jj` assumes that the conflict is fully resolved. This is appropriate
for most graphical merge tools.

Some tools (e.g. `vimdiff`) can present a multi-way diff but
don't resolve conflict themselves. When using such tools, `jj`
can help you by populating the output file with conflict markers
before starting the merge tool (instead of leaving the output file
empty and letting the merge tool fill it in). To do that, set the
`merge-tools.vimdiff.merge-tool-edits-conflict-markers = true` option.

With this option set, if the output file still contains conflict markers
after the conflict is done, `jj` assumes that the conflict was only
partially resolved and parses the conflict markers to get the new state
of the conflict. The conflict is considered fully resolved when there
are no conflict markers left.




# Alternative ways to specify configuration settings

Instead of `~/.jjconfig.toml`, the config settings can be located at
`$XDG_CONFIG_HOME/jj/config.toml` as per the [XDG specification].
It is an error for both of these files to exist.

[XDG specification]: https://specifications.freedesktop.org/basedir-spec/basedir-spec-latest.html

The location of the `jj` config file can also be overriden with the
`JJ_CONFIG` environment variable. If it is not empty, it should contain
the path to a TOML file that will be used instead of any configuration
file in the default locations. For example,

    env JJ_CONFIG=/dev/null jj log       # Ignores any settings specified in the config file.

You can use one or more `--config-toml` options on the command line to
specify additional configuration settings. This overrides settings
defined in config files or environment variables. For example,

    jj --config-toml='ui.color="always"' --config-toml='ui.difftool="kdiff3"' split

Config specified this way must be valid TOML. In paritcular, string
values must be surrounded by quotes. To pass these quotes to `jj`, most
shells require surrounding those quotes with single quotes as shown above.

In `sh`-compatible shells, `--config-toml` can be used to merge entire TOML
files with the config specified in `.jjconfig.toml`:

    jj --config-toml="$(cat extra-config.toml)" log

