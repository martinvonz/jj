# Configuration

These are the config settings available to jj/Jujutsu.

The config settings are located at `~/.jjconfig.toml`. Less common ways to specify `jj` config settings are discussed in a later section.

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

The other thing to remember is that the value of a setting (the part to the 
right of the `=` sign) should be surrounded in quotes if it's a string. 
That's probably enough TOML to keep you out of trouble but the syntax guide is 
very short if you ever need to check.


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


### Editor

The default editor is set via `ui.editor`,
though there are several places to set it. 
The priority is as follows (environment variables are marked with a `$`):

`$JJ_EDITOR` > `ui.editor` > `$VISUAL` > `$EDITOR`

Pico is the default editor in the absence of any other setting but you could 
set it explicitly too.

    ui.editor = "pico"

To use NeoVim instead:

    ui.editor = "nvim"

For GUI editors you possibly need to use a `-w` or `--wait`. Some examples:

    ui.editor = "code -w"       # VS Code
    ui.editor = "bbedit -w"     # BBEdit
    ui.editor = "subl -n -w"    # Sublime Text
    ui.editor = "mate -w"       # TextMate
    ui.editor = "'C:/Program Files/Notepad++/notepad++.exe' -multiInst -notabbar -nosession -noPlugin" # Notepad++
    ui.editor = "idea --temp-project --wait"   #IntelliJ

Obviously, you would only set one line, don't copy them all in!


## Diffing

This setting affects the tool used for editing diffs 
(e.g. `jj split`, `jj amend -i`). 
The default is `meld`.

For example:

    diff-editor = "vimdiff"

When kdiff3 is set via:

    diff-editor = "kdiff3"

further settings are passed on via the following:

    merge-tools.kdiff3.program = "kdiff3"
    merge-tools.kdiff3.edit-args = ["--merge", "--cs", "CreateBakFiles=0"]


## Relative timestamps

    ui.relative-timestamps = true

False by default, but setting to true will change timestamps to be rendered
as `x days/hours/seconds ago` instead of being rendered as a full timestamp.


# Alternative ways to specify configuration settings

Instead of `~/.jjconfig.toml`, the config settings can be located at
`$XDG_CONFIG_HOME/jj/config.toml` as per the [XDG specification].
It is an error for both of these files to exist.

[XDG specification]: https://specifications.freedesktop.org/basedir-spec/basedir-spec-latest.html

The location of the `jj` config file can also be overriden with the `JJ_CONFIG`
environment variable. If it is not empty, it should contain the path to
a TOML file that will be used instead of any configuration file in the
default locations. For example,

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

