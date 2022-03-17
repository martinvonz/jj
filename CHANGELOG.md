# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

* (#111) When undoing an earlier operation, any new commits on top of commits
  from the undone operation will be rebased away. For example, let's say you
  rebase commit A so it becomes a new commit A', and then you create commit B
  on top of A'. If you now undo the rebase operation, commit B will be rebased
  to be on top of A instead. The same logic is used if the repo was modified
  by concurrent operations (so if one operation added B on top of A, and one
  operation rebased A as A', then B would be automatically rebased on top of
  A'). See #111 for more examples.

## [0.3.3] - 2022-03-16

No changes, only trying to get the automated build to work.

## [0.3.2] - 2022-03-16

No changes, only trying to get the automated build to work.

## [0.3.1] - 2022-03-13

### Fixed bugs

 - (#131) Fixed crash when `core.excludesFile` pointed to non-existent file, and
   made leading `~/` in that config expand to `$HOME/`

## [0.3.0] - 2022-03-12

Last release before this changelog started.
