# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Breaking changes

* The `ui.oplog-relative-timestamps` option has been removed. Use the
  `format_time_range()` template alias instead. For details, see
  [the documentation](docs/config.md).

* Implicit concatenation of template expressions has been disabled. Use
  `++` operator, `concat()`, or `separate()` function instead.
  Example: `description ++ "\n"`

* `jj git push` will consider pushing the parent commit only when the
  current commit has no content and no description, such as right after
  a `jj squash`.

* The minimum supported Rust version (MSRV) is now 1.64.0.

* The `heads()` revset function was split up into two functions. `heads()`
  without arguments is now called `visible_heads()`. `heads()` with one argument
  is unchanged.

* The `ui.default-revset` config was renamed to `revsets.log`.

* The `jj sparse` command was split up into `jj sparse list` and
  `jj sparse set`.

* `jj hide` (alias for `jj abandon`) is no longer available. Use `jj abandon`
  instead.

* `jj debug completion`, `jj debug mangen` and `jj debug config-schema` have
  been moved from `jj debug` to `jj util`.

### New features

* `jj git push --deleted` will remove all locally deleted branches from the remote.

* `jj restore` without `--from` works correctly even if `@` is a merge
  commit.

* `jj rebase` now accepts multiple `-s` and `-b` arguments. Revsets with
  multiple commits are allowed with `--allow-large-revsets`.

* `jj git fetch` now supports a `--branch` argument to fetch some of the
  branches only.

* `jj config set` command allows simple config edits like
  `jj config set --repo user.email "somebody@example.com"`

* Added `ui.log-word-wrap` option to wrap `jj log`/`obslog`/`op log` content
  based on terminal width. [#1043](https://github.com/martinvonz/jj/issues/1043)

* Nodes in the (text-based) graphical log output now use a `◉` symbol instead
  of the letter `o`. The ASCII-based graph styles still use `o`.

* Commands that accept a diff format (`jj diff`, `jj interdiff`, `jj show`,
  `jj log`, and `jj obslog`) now accept `--types` to show only the type of file
  before and after.

* `jj describe` now supports `--reset-author` for resetting a commit's author
  to the configured user. `jj describe` also gained a `--no-edit` option to
  avoid opening the editor.

* Added `latest(x[, n])` revset function to select the latest `n` commits.

* Added `conflict()` revset function to select commits with conflicts.

* `jj squash` AKA `jj amend` now accepts a `--message` option to set the
  description of the squashed commit on the command-line.

* The progress display on `jj git clone/fetch` now includes the downloaded size.

* The formatter now supports a "default" color that can override another color
  defined by a parent style.

* `jj obslog` and `jj log` now show abandoned commits as hidden.

* `jj git fetch` and `jj git push` will now use the single defined remote even
  if it is not named "origin".

* `jj git push` now accepts `--branch` and `--change` arguments together.

* `jj git push` now accepts a `-r/--revisions` flag to specify revisions to
  push. All branches pointing to any of the specified revisions will be pushed.
  The flag can be used together with `--branch` and `--change`.

* `jj` with no subcommand now defaults to `jj log` instead of showing help. This
  command can be overridden by setting `ui.default-command`.

* Description tempfiles created via `jj describe` now have the file extension
  `.jjdescription` to help external tooling detect a unique filetype.

* The shortest unique change ID prefixes and commit ID prefixes in `jj log` are
  now shorter within the default log revset. You can override the default by
  setting the `revsets.short-prefixes` config to a different revset.

### Fixed bugs

* Modify/delete conflicts now include context lines
  [#1244](https://github.com/martinvonz/jj/issues/1244).

* It is now possible to modify either side of a modify/delete conflict (any
  change used to be considered a resolution).

* Fixed a bug that could get partially resolved conflicts to be interpreted
  incorrectly.

* `jj git fetch`: when re-adding a remote repository that had been previously
  removed, in some situations the remote branches were not recreated.

* `jj git remote rename`: the git remote references were not rewritten with
  the new name. If a new remote with the old name and containing the same
  branches was added, the remote branches may not be recreated in some cases.

* `jj workspace update-stale` now snapshots the working-copy changes before
  updating to the new working-copy commit.

* It is no longer allowed to create branches at the root commit.

* `git checkout` (without using `jj`) in colocated repo no longer abandons
  the previously checked-out anonymous branch.
  [#1042](https://github.com/martinvonz/jj/issues/1042).

* `jj git fetch` in a colocated repo now abandons branches deleted on the
  remote, just like in a non-colocated repo.
  [#864](https://github.com/martinvonz/jj/issues/864)

## [0.7.0] - 2023-02-16

### Breaking changes

* The minimum supported Rust version (MSRV) is now 1.61.0.

* The `jj touchup` command was renamed to `jj diffedit`.

* The `-i` option to `jj restore` was removed in favor of new `--from`/`--to`
  options to `jj diffedit`.

* To report the situation when a change id corresponds to multiple visible
  commits, `jj log` now prints the change id in red and puts `??` after it.
  Previously, it printed the word "divergent".

* `jj log` prefixes commit descriptions with "(empty)" when they contain no
  change compared to their parents.

* The `author`/`committer` templates now display both name and email. Use
  `author.name()`/`committer.name()` to extract the name.

* Storage of the "HEAD@git" reference changed and can now have conflicts.
  Operations written by a new `jj` binary will have a "HEAD@git" reference that
  is not visible to older binaries.

* The `description` template keyword is now empty if no description set.
  Use `if(description, description, "(no description set)\n")` to get back
  the previous behavior.

* The `template.log.graph` and `template.commit_summary` config keys were
  renamed to `templates.log` and `templates.commit_summary` respectively.

* If a custom `templates.log` template is set, working-copy commit will
  no longer be highlighted automatically. Wrap your template with
  `label(if(current_working_copy, "working_copy"), ...)` to label the
  working-copy entry.

* The `ui.relative-timestamps` option has been removed. Use the
  `format_timestamp()` template alias instead. For details on showing relative
  timestamps in `jj log` and `jj show`, see [the documentation](docs/config.md).

* `jj op log` now shows relative timestamps by default. To disable, set
  `ui.oplog-relative-timestamps` to `false`.

* The global `--no-commit-working-copy` is now called `--ignore-working-copy`.

* The `diff.format` config option is now called `ui.diff.format`. The old name
  is still supported for now.

* `merge-tools.<name>.edit-args` now requires `$left`/`$right` parameters.
  The default is `edit-args = ["$left", "$right"]`.

* The builtin `jj update` and `jj up` aliases for `jj checkout` have been
  deleted.

* Change IDs are now rendered using letters from the end of the alphabet (from
  'z' through 'k') instead of the usual hex digits ('0' through '9' and 'a'
  through 'f'). This is to clarify the distinction between change IDs and commit
  IDs, and to allow more efficient lookup of unique prefixes. This change
  doesn't affect the storage format; existing repositories will remain usable.

### New features

* The default log format now uses the committer timestamp instead of the author
  timestamp.

* `jj log --summary --patch` now shows both summary and diff outputs.

* `jj git push` now accepts multiple `--branch`/`--change` arguments

* `jj config list` command prints values from config and `config edit` opens
  the config in an editor.

* `jj debug config-schema` command prints out JSON schema for the jj TOML config
  file format.

* `jj resolve --list` can now describe the complexity of conflicts.

* `jj resolve` now notifies the user of remaining conflicts, if any, on success.
  This can be prevented by the new `--quiet` option.

* Per-repository configuration is now read from `.jj/repo/config.toml`.

* Background colors, bold text, and underlining are now supported. You can set
  e.g. `color.error = { bg = "red", bold = true, underline = true }` in your
  `~/.jjconfig.toml`.

* The `empty` condition in templates is true when the commit makes no change to
  the three compared to its parents.

* `branches([needle])` revset function now takes `needle` as an optional
  argument and matches just the branches whose name contains `needle`.

* `remote_branches([branch_needle[, remote_needle]])` now takes `branch_needle`
  and `remote_needle` as optional arguments and matches just the branches whose
  name contains `branch_needle` and remote contains `remote_needle`.

* `jj git fetch` accepts repeated `--remote` arguments.

* Default remotes can be configured for the `jj git fetch` and `jj git push`
  operations ("origin" by default) using the `git.fetch` and `git.push`
  configuration entries. `git.fetch` can be a list if multiple remotes must
  be fetched from.

* `jj duplicate` can now duplicate multiple changes in one go. This preserves
  any parent-child relationships between them. For example, the entire tree of
  descendants of `abc` can be duplicated with `jj duplicate abc:`.

* `jj log` now highlights the shortest unique prefix of every commit and change
  id and shows the rest in gray. To customize the length and style, use the
  `format_short_id()` template alias. For details, see
  [the documentation](docs/config.md).

* `jj print` was renamed to `jj cat`. `jj print` remains as an alias.

* In content that goes to the terminal, the ANSI escape byte (0x1b) is replaced
  by a "␛" character. That prevents them from interfering with the ANSI escapes
  jj itself writes.

* `jj workspace root` prints the root path of the current workspace.

* The `[alias]` config section was renamed to `[aliases]`. The old name is
  still accepted for backwards compatibility for some time.

* Commands that draw an ASCII graph (`jj log`, `jj op log`, `jj obslog`) now
  have different styles available by setting e.g. `ui.graph.style = "curved"`.

* `jj split` accepts creating empty commits when given a path. `jj split .`
  inserts an empty commit between the target commit and its children if any,
  and `jj split any-non-existent-path` inserts an empty commit between the
  target commit and its parents.

* Command arguments to `ui.diff-editor`/`ui.merge-editor` can now be specified
  inline without referring to `[merge-tools]` table.

* `jj rebase` now accepts a new `--allow-large-revsets` argument that allows the
  revset in the `-d` argument to expand to several revisions. For example,
  `jj rebase -s B -d B- -d C` now works even if `B` is a merge commit.

* `jj new` now also accepts a `--allow-large-revsets` argument that behaves
  similarly to `jj rebase --allow-large-revsets`.

* `jj new --insert-before` inserts the new commit between the target commit and
  its parents.

* `jj new --insert-after` inserts the new commit between the target commit and
  its children.

* `author`/`committer` templates now support `.username()`, which leaves out the
  domain information of `.email()`.

* It is now possible to change the author format of `jj log` with the
  `format_short_signature()` template alias. For details, see
  [the documentation](docs/config.md).

* Added support for template aliases. New symbols and functions can be
  configured by `template-aliases.<name> = <expression>`. Be aware that
  the template syntax isn't documented yet and is likely to change.

* The `ui.diff-instructions` config setting can be set to `false` to inhibit the
  creation of the `JJ-INSTRUCTIONS` file as part of diff editing.

### Fixed bugs

* When sharing the working copy with a Git repo, we used to forget to export
  branches to Git when only the working copy had changed. That's now fixed.

* Commit description set by `-m`/`--message` is now terminated with a newline
  character, just like descriptions set by editor are.

* The `-R`/`--repository` path must be a valid workspace directory. Its
  ancestor directories are no longer searched.

* Fixed a crash when trying to access a commit that's never been imported into
  the jj repo from a Git repo. They will now be considered as non-existent if
  referenced explicitly instead of crashing.

* Fixed handling of escaped characters in .gitignore (only keep trailing spaces
  if escaped properly).

* `jj undo` now works after `jj duplicate`.

* `jj duplicate` followed by `jj rebase` of a tree containing both the original
  and duplicate commit no longer crashes. The fix should also resolve any remaining
  instances of https://github.com/martinvonz/jj/issues/27.

* Fix the output of `jj debug completion --help` by reversing fish and zsh text.

* Fixed edge case in `jj git fetch` when a pruned branch is a prefix of another
  branch.

### Contributors

Thanks to the people who made this release happen!

 * Aleksandr Mikhailov (@AM5800)
 * Augie Fackler (@durin42)
 * Benjamin Saunders (@Ralith)
 * Daniel Ploch (@torquestomp)
 * Danny Hooper (@hooper)
 * David Barnett (@dbarnett)
 * Glen Choo (@chooglen)
 * Herby Gillot (@herbygillot)
 * Ilya Grigoriev (@ilyagr)
 * Luke Granger-Brown (@lukegb)
 * Martin von Zweigbergk (@martinvonz)
 * Michael Forster (@MForster)
 * Philip Metzger (@PhilipMetzger)
 * Ruben Slabbert (@rslabbert)
 * Samuel Tardieu (@samueltardieu)
 * Tal Pressman (@talpr)
 * Vamsi Avula (@avamsi)
 * Waleed Khan (@arxanas)
 * Yuya Nishihara (@yuja)

## [0.6.1] - 2022-12-05

No changes, only changed to a released version of the `thrift` crate dependency.

## [0.6.0] - 2022-12-05

### Breaking changes

* Dropped candidates set argument from `description(needle)`, `author(needle)`,
  `committer(needle)`, `merges()` revsets. Use `x & description(needle)`
  instead.

* Adjusted precedence of revset union/intersection/difference operators.
  `x | y & z` is now equivalent to `x | (y & z)`.

* Support for open commits has been dropped. The `ui.enable-open-commits` config
  that was added in 0.5.0 is no longer respected. The `jj open/close` commands
  have been deleted.

* `jj commit` is now a separate command from `jj close` (which no longer
  exists). The behavior has changed slightly. It now always asks for a
  description, even if there already was a description set. It now also only
  works on the working-copy commit (there's no `-r` argument).

* If a workspace's working-copy commit has been updated from another workspace,
  most commands in that workspace will now fail. Use the new
  `jj workspace update-stale` command to update the workspace to the new
  working-copy commit. (The old behavior was to automatically update the
  workspace.)

### New features

* Commands with long output are paginated.
  [#9](https://github.com/martinvonz/jj/issues/9)

* The new `jj git remote rename` command allows git remotes to be renamed
  in-place.

* The new `jj resolve` command allows resolving simple conflicts with
  an external 3-way-merge tool.

* `jj git push` will search `@-` for branches to push if `@` has none.

* The new revset function `file(pattern..)` finds commits modifying the
  paths specified by the `pattern..`.

* The new revset function `empty()` finds commits modifying no files.

* Added support for revset aliases. New symbols and functions can be configured
  by `revset-aliases.<name> = <expression>`.

* It is now possible to specify configuration options on the command line
  with the new `--config-toml` global option.

* `jj git` subcommands will prompt for credentials when required for HTTPS
  remotes rather than failing.
  [#469](https://github.com/martinvonz/jj/issues/469)

* Branches that have a different target on some remote than they do locally are
  now indicated by an asterisk suffix (e.g. `main*`) in `jj log`.
  [#254](https://github.com/martinvonz/jj/issues/254)

* The commit ID was moved from first on the line in `jj log` output to close to
  the end. The goal is to encourage users to use the change ID instead, since
  that is generally more convenient, and it reduces the risk of creating
  divergent commits.

* The username and hostname that appear in the operation log are now
  configurable via config options `operation.username` and `operation.hostname`.

* `jj git` subcommands now support credential helpers.

* `jj log` will warn if it appears that the provided path was meant to be a
  revset.

* The new global flag `-v/--verbose` will turn on debug logging to give
  some additional insight into what is happening behind the scenes.
  Note: This is not comprehensively supported by all operations yet.

* `jj log`, `jj show`, and `jj obslog` now all support showing relative
  timestamps by setting `ui.relative-timestamps = true` in the config file.

### Fixed bugs

* A bug in the export of branches to Git caused spurious conflicted branches.
  This typically occurred when running in a working copy colocated with Git
  (created by running `jj init --git-dir=.`).
  [#463](https://github.com/martinvonz/jj/issues/463)

* When exporting branches to Git, we used to fail if some branches could not be
  exported (e.g. because Git doesn't allow a branch called `main` and another
  branch called `main/sub`). We now print a warning about these branches
  instead.
  [#493](https://github.com/martinvonz/jj/issues/493)

* If you had modified branches in jj and also modified branches in conflicting
  ways in Git, `jj git export` used to overwrite the changes you made in Git.
  We now print a warning about these branches instead.

* `jj edit root` now fails gracefully.

* `jj git import` used to abandon a commit if Git branches and tags referring
  to it were removed. We now keep it if a detached HEAD refers to it.

* `jj git import` no longer crashes when all Git refs are removed.

* Git submodules are now ignored completely. Earlier, files present in the
  submodule directory in the working copy would become added (tracked), and
  later removed if you checked out another commit. You can now use `git` to
  populate the submodule directory and `jj` will leave it alone.

* Git's GC could remove commits that were referenced from jj in some cases. We
  are now better at adding Git refs to prevent that.
  [#815](https://github.com/martinvonz/jj/issues/815)

* When the working-copy commit was a merge, `jj status` would list only the
  first parent, and the diff summary would be against that parent. The output
  now lists all parents and the diff summary is against the auto-merged parents.

### Contributors

Thanks to the people who made this release happen!

 * Martin von Zweigbergk (@martinvonz)
 * Benjamin Saunders (@Ralith)
 * Yuya Nishihara (@yuja)
 * Glen Choo (@chooglen)
 * Ilya Grigoriev (@ilyagr)
 * Ruben Slabbert (@rslabbert)
 * Waleed Khan (@arxanas)
 * Sean E. Russell (@xxxserxxx)
 * Pranay Sashank (@pranaysashank)
 * Luke Granger-Brown (@lukegb)


## [0.5.1] - 2022-10-17

No changes (just trying to get automated GitHub release to work).

## [0.5.0] - 2022-10-17

### Breaking changes

* Open commits are now disabled by default. That means that `jj checkout` will
  always create a new change on top of the specified commit and will let you
  edit that in the working copy. Set `ui.enable-open-commits = true` to restore
  the old behavior and let us know that you did so we know how many people
  prefer the workflow with open commits.

* `jj [op] undo` and `jj op restore` used to take the operation to undo or
  restore to as an argument to `-o/--operation`. It is now a positional
  argument instead (i.e. `jj undo -o abc123` is now written `jj undo abc123`).

* An alias that is not configured as a string list (e.g. `my-status = "status"`
  instead of `my-status = ["status"]`) is now an error instead of a warning.

* `jj log` now defaults to showing only commits that are not on any remote
  branches (plus their closest commit on the remote branch for context). This
  set of commits can be overridden by setting `ui.default-revset`. Use
  `jj log -r 'all()'` for the old behavior. Read more about revsets
  [here](https://github.com/martinvonz/jj/blob/main/docs/revsets.md).
  [#250](https://github.com/martinvonz/jj/issues/250)

* `jj new` now always checks out the new commit (used to be only if the parent
  was `@`).

* `jj merge` now checks out the new commit. The command now behaves exactly
  like `jj new`, except that it requires at least two arguments.

* When the working-copy commit is abandoned by `jj abandon` and the parent
  commit is open, a new working-copy commit will be created on top (the open
  parent commit used to get checked out).

* `jj branch` now uses subcommands like `jj branch create` and
  `jj branch forget` instead of options like `jj branch --forget`.
  [#330](https://github.com/martinvonz/jj/issues/330)

* The [`$NO_COLOR` environment variable](https://no-color.org/) no longer
  overrides the `ui.color` configuration if explicitly set.

* `jj edit` has been renamed to `jj touchup`, and `jj edit` is now a new command
  with different behavior. The new `jj edit` lets you edit a commit in the
  working copy, even if the specified commit is closed.

* `jj git push` no longer aborts if you attempt to push an open commit (but it
  now aborts if a commit does not have a description).

* `jj git push` now pushes only branches pointing to the `@` by default. Use
  `--all` to push all branches.

* The `checkouts` template keyword is now called `working_copies`, and
  `current_checkout` is called `current_working_copy`.

### New features

* The new `jj interdiff` command compares the changes in commits, ignoring
  changes from intervening commits.

* `jj rebase` now accepts a `--branch/-b <revision>` argument, which can be used
  instead of `-r` or `-s` to specify which commits to rebase. It will rebase the
  whole branch, relative to the destination. The default mode has changed from
  `-r @` to `-b @`.

* The new `jj print` command prints the contents of a file in a revision.

* The new `jj git remotes list` command lists the configured remotes and their
  URLs.
  [#243](https://github.com/martinvonz/jj/issues/243)

* `jj move` and `jj squash` now lets you limit the set of changes to move by
  specifying paths on the command line (in addition to the `--interactive`
  mode). For example, use `jj move --to @-- foo` to move the changes to file
  (or directory) `foo` in the working copy to the grandparent commit.

* When `jj move/squash/unsquash` abandons the source commit because it became
  empty and both the source and the destination commits have non-empty
  descriptions, it now asks for a combined description. If either description
  was empty, it uses the other without asking.

* `jj split` now lets you specify on the CLI which paths to include in the first
  commit. The interactive diff-editing is not started when you do that.

* Sparse checkouts are now supported. In fact, all working copies are now
  "sparse", only to different degrees. Use the `jj sparse` command to manage
  the paths included in the sparse checkout.

* Configuration is now also read from `~/.jjconfig.toml`.

* The `$JJ_CONFIG` environment variable can now point to a directory. If it
  does, all files in the directory will be read, in alphabetical order.

* The `$VISUAL` environment is now respected and overrides `$EDITOR`. The new
  `ui.editor` config has higher priority than both of them. There is also a new
  `$JJ_EDITOR` environment variable, which has even higher priority than the
  config.

* You can now use `-` and `+` in revset symbols. You used to have to quote
  branch names like `my-feature` in nested quotes (outer layer for your shell)
  like `jj co '"my-feature"'`. The quoting is no longer needed.

* The new revset function `connected(x)` is the same as `x:x`.

* The new revset function `roots(x)` finds commits in the set that are not
  descendants of other commits in the set.

* ssh-agent is now detected even if `$SSH_AGENT_PID` is not set (as long as
  `$SSH_AUTH_SOCK` is set). This should help at least macOS users where
  ssh-agent is launched by default and only `$SSH_AUTH_SOCK` is set.

* When importing from a git, any commits that are no longer referenced on the
  git side will now be abandoned on the jj side as well. That means that
  `jj git fetch` will now abandon unreferenced commits and rebase any local
  changes you had on top.

* `jj git push` gained a `--change <revision>` argument. When that's used, it
  will create a branch named after the revision's change ID, so you don't have
  to create a branch yourself. By default, the branch name will start with
  `push-`, but this can be overridden by the `push.branch-prefix` config
  setting.

* `jj git push` now aborts if you attempt to push a commit without a
  description or with the placeholder "(no name/email configured)" values for
  author/committer.

* Diff editor command arguments can now be specified by config file.
  Example:

      [merge-tools.kdiff3]
      program = "kdiff3"
      edit-args = ["--merge", "--cs", "CreateBakFiles=0"]

* `jj branch` can accept any number of branches to update, rather than just one.

* Aliases can now call other aliases.

* `jj log` now accepts a `--reversed` option, which will show older commits
  first.

* `jj log` now accepts file paths.

* `jj obslog` now accepts `-p`/`--patch` option, which will show the diff
  compared to the previous version of the change.

* The "(no name/email configured)" placeholder value for name/email will now be
  replaced if once you modify a commit after having configured your name/email.

* Color setting can now be overridden by `--color=always|never|auto` option.

* `jj checkout` now lets you specify a description with `--message/-m`.

* `jj new` can now be used for creating merge commits. If you pass more than
  one argument to it, the new commit will have all of them as parents.

### Fixed bugs

* When rebasing a conflict where one side modified a file and the other side
  deleted it, we no longer automatically resolve it in favor of the modified
  content (this was a regression from commit c0ae4b16e8c4).

* Errors are now printed to stderr (they used to be printed to stdout).

* Updating the working copy to a commit where a file's executable bit changed
  but the contents was the same used to lead to a crash. That has now been
  fixed.

* If one side of a merge modified a directory and the other side deleted it, it
  used to be considered a conflict. The same was true if both sides added a
  directory with different files in. They are now merged as if the missing
  directory had been empty.

* When using `jj move` to move part of a commit into an ancestor, any branches
  pointing to the source commit used to be left on a hidden intermediate commit.
  They are now correctly updated.

* `jj untrack` now requires at least one path (allowing no arguments was a UX
  bug).

* `jj rebase` now requires at least one destination (allowing no arguments was a
  UX bug).

* `jj restore --to <rev>` now restores from the working copy (it used to restore
  from the working copy's parent).

* You now get a proper error message instead of a crash when `$EDITOR` doesn't
  exist or exits with an error.

* Global arguments, such as `--at-op=<operation>`, can now be passed before
  an alias.

* Fixed relative path to the current directory in output to be `.` instead of
  empty string.

* When adding a new workspace, the parent of the current workspace's current
  checkout will be checked out. That was always the intent, but the root commit
  was accidentally checked out instead.

* When checking out a commit, the previous commit is no longer abandoned if it
  has a non-empty description.

* All commands now consistently snapshot the working copy (it was missing from
  e.g. `jj undo` and `jj merge` before).

## [0.4.0] - 2022-04-02

### Breaking changes

* Dropped support for config in `~/.jjconfig`. Your configuration is now read
  from `<config dir>/jj/config.toml`, where `<config dir>` is
  `${XDG_CONFIG_HOME}` or `~/.config/` on Linux,
  `~/Library/Application Support/` on macOS, and `~\AppData\Roaming\` on
  Windows.

### New features

* You can now set an environment variable called `$JJ_CONFIG` to a path to a
  config file. That will then be read instead of your regular config file. This
  is mostly intended for testing and scripts.

* The [standard `$NO_COLOR` environment variable](https://no-color.org/) is now
  respected.

* `jj new` now lets you specify a description with `--message/-m`.

* When you check out a commit, the old commit no longer automatically gets
  abandoned if it's empty and has descendants, it only gets abandoned if it's
  empty and does not have descendants.

* When undoing an earlier operation, any new commits on top of commits from the
  undone operation will be rebased away. For example, let's say you rebase
  commit A so it becomes a new commit A', and then you create commit B on top of
  A'. If you now undo the rebase operation, commit B will be rebased to be on
  top of A instead. The same logic is used if the repo was modified by
  concurrent operations (so if one operation added B on top of A, and one
  operation rebased A as A', then B would be automatically rebased on top of
  A'). See #111 for more examples.
  [#111](https://github.com/martinvonz/jj/issues/111)

* `jj log` now accepts `-p`/`--patch` option.

### Fixed bugs

* Fixed crash on `jj init --git-repo=.` (it almost always crashed).

* When sharing the working copy with a Git repo, the automatic importing and
  exporting (sometimes?) didn't happen on Windows.

## [0.3.3] - 2022-03-16

No changes, only trying to get the automated build to work.

## [0.3.2] - 2022-03-16

No changes, only trying to get the automated build to work.

## [0.3.1] - 2022-03-13

### Fixed bugs

 - Fixed crash when `core.excludesFile` pointed to nonexistent file, and made
   leading `~/` in that config expand to `$HOME/`
   [#131](https://github.com/martinvonz/jj/issues/131)

## [0.3.0] - 2022-03-12

Last release before this changelog started.
