# How to Contribute


## Policies

We'd love to accept your patches and contributions to this project. There are
just a few small guidelines you need to follow.

### Contributor License Agreement

Contributions to this project must be accompanied by a Contributor License
Agreement. You (or your employer) retain the copyright to your contribution;
this simply gives us permission to use and redistribute your contributions as
part of the project. Head over to <https://cla.developers.google.com/> to see
your current agreements on file or to sign a new one.

You generally only need to submit a CLA once, so if you've already submitted one
(even if it was for a different project), you probably don't need to do it
again.

### Code reviews

All submissions, including submissions by project members, require review. We
use GitHub pull requests for this purpose. Consult
[GitHub Help](https://help.github.com/articles/about-pull-requests/) for more
information on using pull requests.

Unlike many GitHub projects (but like many VCS projects), we care more about the
contents of commits than about the contents of PRs. We review each commit
separately, and we don't squash-merge the PR (so please manually squash any
fixup commits before sending for review).

Each commit should ideally do one thing. For example, if you need to refactor a
function in order to add a new feature cleanly, put the refactoring in one
commit and the new feature in a different commit. If the refactoring itself
consists of many parts, try to separate out those into separate commits. You can
use `jj split` to do it if you didn't realize ahead of time how it should be
split up. Include tests and documentation in the same commit as the code they
test and document.

The commit message should describe the changes in the commit;
the PR description can even be empty, but feel free to include a personal
message. We start the commit message with `<topic>: `  and don't use
[conventional commits](https://www.conventionalcommits.org/en/v1.0.0/). This means if
you modified a command in the CLI, use its name as the topic, e.g.
`next/prev: <your-modification>` or `conflicts: <your-modification>`. We don't
currently have a specific guidelines on what to write in the topic field, but
the reviewers will help you provide a topic if you have difficulties choosing
it. [How to Write a Git Commit Message](https://cbea.ms/git-commit/) is a good
guide if you're new to writing good commit messages. We are not particularly
strict about the style, but please do explain the reason for the change unless
it's obvious.

When you address comments on a PR, don't make the changes in a commit on top (as
is typical on GitHub). Instead, please make the changes in the appropriate
commit. You can do that by creating a new commit on top of the initial commit
 (`jj new <commit>`) and then squash in the changes when you're done (`jj squash`).
`jj git push`
will automatically force-push the bookmark.

When your first PR has been approved, we typically give you contributor access,
so you can address any remaining minor comments and then merge the PR yourself
when you're ready. If you realize that some comments require non-trivial
changes, please ask your reviewer to take another look.

To avoid conflicts of interest, please don't merge a PR that has only been
approved by someone from the same organization. Similarly, as a reviewer,
there is no need to approve your coworkers' PRs, since the author should await
an approval from someone else anyway. It is of course still appreciated if you
review and comment on their PRs. Also, if the PR seems completely unrelated to
your company's interests, do feel free to approve it.

### Community Guidelines

This project follows [Google's Open Source Community
Guidelines](https://opensource.google/conduct/).

## Contributing large patches

Before sending a PR for a large change which designs/redesigns or reworks an
existing component, we require an architecture review from  multiple
stakeholders, which we do with [Design Docs](design_docs.md), see the
[process here](design_docs.md#process).

## Contributing to the documentation

We appreciate [bug
reports](https://github.com/martinvonz/jj/issues/new?template=bug_report.md)
about any problems, however small, lurking in [our documentation
website](https://martinvonz.github.io/jj/prerelease) or in the `jj help
<command>` docs. If a part of the bug report template does not apply, you can
just delete it.

Before reporting a problem with the documentation website, we'd appreciate it if
you could check that the problem still exists in the "prerelease" version of the
documentation (as opposed to the docs for one of the released versions of `jj`).
You can use the version switcher in the top-left of the website to do so.

If you are willing to make a PR fixing a documentation problem, even better!

The documentation website sources are Markdown files located in the [`docs/`
directory](https://github.com/martinvonz/jj/tree/main/docs). You do not need to
know Rust to work with them. See below for [instructions on how to preview the
HTML docs](#previewing-the-html-documentation) as you edit the Markdown files.
Doing so is optional, but recommended.

The `jj help` docs are sourced from the "docstring" comments inside the Rust
sources, currently from the [`cli/src/commands`
directory](https://github.com/martinvonz/jj/tree/main/cli/src/commands). Working
on them requires setting up a Rust development environment, as described
below, and may occasionally require adjusting a test.


## Learning Rust

In addition to the [Rust Book](https://doc.rust-lang.org/book/) and the other
excellent resources at <https://www.rust-lang.org/learn>, we recommend the
["Comprehensive Rust" mini-course](https://google.github.io/comprehensive-rust/)
for an overview, especially if you are familiar with C++.

## Setting up a development environment

To develop `jj`, the mandatory steps are simply
to [install Rust](https://www.rust-lang.org/tools/install) (the default
installer options are fine), clone the repository, and use `cargo build`
, `cargo fmt`,
`cargo clippy --workspace --all-targets`, and
`cargo test --workspace`. If you are preparing a PR, there are some additional
recommended steps.

### Summary

One-time setup:

    rustup toolchain add nightly  # wanted for 'rustfmt'
    rustup toolchain add 1.76     # also specified in Cargo.toml
    cargo install cargo-insta
    cargo install cargo-watch
    cargo install cargo-nextest

During development (adapt according to your preference):

    cargo watch --ignore '.jj/**' -s \
      'cargo clippy --workspace --all-targets \
       && cargo +1.76 check --workspace --all-targets'
    cargo +nightly fmt # Occasionally
    cargo nextest run --workspace # Occasionally
    cargo insta test --workspace --test-runner nextest # Occasionally

WARNING: Build artifacts from debug builds and especially from repeated
invocations of `cargo test` can quickly take up 10s of GB of disk space.
Cargo will happily use up your entire hard drive. If this happens, run
`cargo clean`.

### Explanation

These are listed roughly in order of decreasing importance.

1. Nearly any change to `jj`'s CLI will require writing or updating snapshot
   tests that use the [`insta`](https://insta.rs/) crate. To make this
   convenient, install the `cargo-insta` binary.
   Use `cargo insta test --workspace` to run tests,
   and `cargo insta review --workspace` to update the snapshot tests.
   The `--workspace` flag is needed to run the tests on all crates; by default,
   only the crate in the current directory is tested.

2. GitHub CI checks require that the code is formatted with the *nightly*
   version of `rustfmt`. To do this on your computer, install the nightly
   toolchain and use `cargo +nightly fmt`.

3. Your code will be rejected if it cannot be compiled with the minimal
   supported version of Rust ("MSRV"). Currently, `jj` follows a rather
   casual MSRV policy: "The current `rustc` stable version, minus one."
   As of this writing, that version is **1.76.0**.

4. Your code needs to pass `cargo clippy`. You can also
   use `cargo +nightly clippy` if you wish to see more warnings.

5. You may also want to install and use `cargo-watch`. In this case, you should
   exclude `.jj`. directory from the filesystem watcher, as it gets updated on
   every `jj log`.

6. To run tests more quickly, use `cargo nextest run --workspace`. To
   use `nextest` with `insta`, use `cargo insta test --workspace
   --test-runner nextest`.

   On Linux, you may be able to speed up `nextest` even further by using
   the `mold` linker, as explained below.

### Using `mold` for faster tests on Linux

On a machine with a multi-core CPU, one way to speed up
`cargo nextest` on Linux is to use the multi-threaded [`mold`
linker](https://github.com/rui314/mold). This linker may help
if, currently, your CPU is underused while Rust is linking test
binaries. Before proceeding with `mold`, you can check whether this is
an issue worth solving using a system monitoring tool such as `htop`.

`mold` is packaged for many distributions. On Debian, for example,
`sudo apt install mold` should just work.

A simple way to use `mold` is via the `-run` option, e.g.:

```shell
mold -run cargo insta test --workspace --test-runner nextest
```

There will be no indication that a different linker is used, except for
higher CPU usage while linking and, hopefully, faster completion. You
can verify that `mold` was indeed used by running
`readelf -p .comment target/debug/jj`.

There are also ways of having Rust use `mold` by default, see the ["How
to use" instructions](https://github.com/rui314/mold#how-to-use).

On recent versions of MacOS, the default linker Rust uses is already
multi-threaded. It should use all the CPU cores without any configuration.

### Editor setup

#### Visual Studio Code

We recommend at least these settings:

```js
{
    "files.insertFinalNewline": true,
    "files.trimTrailingWhitespace": true,
    "[rust]": {
        "files.trimTrailingWhitespace": false
    }
}
```

#### Zed

```js
// .zed/settings.json
{
  "ensure_final_newline_on_save": true,
  "remove_trailing_whitespace_on_save": true,

  "languages": {
    // We don't use a formatter for Markdown files, so format_on_save would just
    // mess with others' docs
    "Markdown": { "format_on_save": "off" }
    "Rust": {
      "format_on_save": "on",
      // Avoid removing trailing spaces within multi-line string literals
      "remove_trailing_whitespace_on_save": false
    }
  },

  "lsp": {
    "rust-analyzer": {
      "initialization_options": {
        // If you are working on docs and don't need `cargo check`, uncomment
        // this option:
        //
        //   "checkOnSave": false,

        // Use nightly `rustfmt`, equivalent to `cargo +nightly fmt`
        "rustfmt": { "extraArgs": ["+nightly"] }
      }
    }
  }
}
```

## Previewing the HTML documentation

The documentation for `jj` is automatically published online at
<https://martinvonz.github.io/jj/>.

When editing documentation, you should check your changes locally — especially
if you are adding a new page, or doing a major rewrite.

### Install `uv`

The only thing you need is [`uv`][uv] (version 0.5.1 or newer).

`uv` is a Python project manager written in Rust. It will fetch the right Python
version and the dependencies needed to build the docs. Install it like so:

[uv]: https://docs.astral.sh/uv/

=== "macOS/Linux"

    ``` { .shell .copy }
    curl -LsSf https://astral.sh/uv/install.sh | sh
    ```

    !!! note
        If you don't have `~/.local/bin` in your `PATH`, the installer will
        modify your shell profile. To avoid it:

        ``` { .shell .copy }
        curl -LsSf https://astral.sh/uv/install.sh | env INSTALLER_NO_MODIFY_PATH=1 sh
        ```

=== "Windows"

    ``` { .shell .copy }
    powershell -ExecutionPolicy ByPass -c "irm https://astral.sh/uv/install.ps1 | iex"
    ```

=== "Homebrew"

    ``` { .shell .copy }
    brew install uv
    ```

=== "Cargo"

    ``` { .shell .copy }
    # This might take a while
    cargo install --git https://github.com/astral-sh/uv uv
    ```

=== "Other options"

    * Directly download the binaries from GitHub: [uv releases](https://github.com/astral-sh/uv/releases).
    * Even more options: [Installing uv](https://docs.astral.sh/uv/getting-started/installation/).

### Build the docs

To build the docs, run from the root of the `jj` repository:

``` { .shell .copy }
uv run mkdocs serve
```

Open <http://127.0.0.1:8000> in your browser to see the docs.

As you edit the `.md` files in `docs/`, the website should be rebuilt and
reloaded in your browser automatically.

!!! note "If the docs are not updating"
    Check the terminal from which you ran `uv run mkdocs serve` for any build
    errors or warnings. Warnings about `"GET /versions.json HTTP/1.1" code 404`
    are expected and harmless.

## Building the entire website

!!! tip
    Building the entire website is not usually necessary. If you are editing
    documentation, the previous section is enough.

    These instructions are relevant if you are working on the versioning of the
    documentation that we currently do with `mike`.

The full `jj` website includes the documentation for several `jj` versions
(`prerelease`, latest release, and the older releases). The top-level
URL <https://martinvonz.github.io/jj> redirects to
<https://martinvonz.github.io/jj/latest>, which in turn redirects to
the docs for the last stable version.

The different versions of documentation are managed and deployed with
[`mike`](https://github.com/jimporter/mike), which can be run with
`uv run mike`.

On a POSIX system or WSL, one way to build the entire website is as follows (on
Windows, you'll need to understand and adapt the shell script):

1. Check out `jj` as a co-located `jj + git` repository (`jj clone --colocate`),
cloned from your fork of `jj` (e.g. `github.com/jjfan/jj`). You can also use a
pure Git repo if you prefer.

2. Make sure `github.com/jjfan/jj` includes the `gh-pages` bookmark of the jj repo
and run `git fetch origin gh-pages`.

3. Go to the GitHub repository settings, enable GitHub Pages, and configure them
to use the `gh-pages` bookmark (this is usually the default).

4. Install `uv` as explained in [Previewing the HTML
documentation](#previewing-the-html-documentation), and run the same `sh` script
that is used in GitHub CI (details below):

    ```shell
    .github/scripts/docs-build-deploy 'https://jjfan.github.io/jj/'\
        prerelease main --push
    ```

    This should build the version of the docs from the current commit,
    deploy it as a new commit to the `gh-pages` bookmark,
    and push the `gh-pages` bookmark to the origin.

5. Now, you should be able to see the full website, including your latest changes
to the `prerelease` version, at `https://jjfan.github.io/jj/prerelease/`.

6. (Optional) The previous steps actually only rebuild
`https://jjfan.github.io/jj/prerelease/` and its alias
`https://jjfan.github.io/jj/main/`. If you'd like to test out version switching
back and forth, you can also rebuild the docs for the latest release as follows.

    ```shell
    jj new v1.33.1  # Let's say `jj 1.33.1` is the currently the latest release
    .github/scripts/docs-build-deploy 'https://jjfan.github.io/jj/'\
        v1.33.1 latest --push
    ```

7. (Optional) When you are done, you may want to reset the `gh-bookmarks` to the
same spot as it is in the upstream. If you configured the `upstream` remote,
this can be done with:

    ```shell
    # This will LOSE any changes you made to `gh-pages`
    jj git fetch --remote upstream
    jj bookmark set gh-pages -r gh-pages@upstream
    jj git push --remote origin --bookmark gh-pages
    ```

    If you want to preserve some of the changes you made, you can do `jj bookmark
    set my-changes -r gh-pages` BEFORE running the above commands.

### Explanation of the `docs-build-deploy` script

The script sets up the `site_url` mkdocs config to
`'https://jjfan.github.io/jj/'`. If this config does not match the URL
where you loaded the website, some minor website features (like the
version switching widget) will have reduced functionality.

Then, the script passes the rest of its arguments to `uv run mike
deploy`, which does the rest of the job. Run `uv run mike help deploy` to
find out what the arguments do.

If you need to do something more complicated, you can use `uv run mike
...` commands. You can also edit the `gh-pages` bookmark directly, but take care
to avoid files that will be overwritten by future invocations of `mike`. Then,
you can submit a PR based on the `gh-pages` bookmark of
<https://martinvonz.github.com/jj> (instead of the usual `main` bookmark).


## Modifying protobuffers (this is not common)

 Occasionally, you may need to change the `.proto` files that define jj's data
 storage format. In this case, you will need to add a few steps to the above
 workflow.

 - Install the `protoc` compiler. This usually means either `apt-get install
   protobuf-compiler` or downloading [an official release]. The
   `prost` [library docs] have additional advice.
 - Run `cargo run -p gen-protos` regularly (or after every edit to a `.proto`
   file). This is the same as running `cargo run` from `lib/gen-protos`. The
   `gen-protos` binary will use the `prost-build` library to compile the
   `.proto` files into `.rs` files.
 - If you are adding a new `.proto` file, you will need to edit the list of
   these files in `lib/gen-protos/src/main.rs`.

[an official release]: https://github.com/protocolbuffers/protobuf/releases
[library docs]: https://docs.rs/prost-build/latest/prost_build/#sourcing-protoc

 The `.rs` files generated from `.proto` files are included in the repository,
 and there is a GitHub CI check that will complain if they do not match.

## Profiling

One easy-to-use sampling profiler
is [samply](https://github.com/mstange/samply). For example:
```shell
cargo install samply
samply record jj diff
```
Then just open the link it prints.

Another option is to use the instrumentation we've added manually (using
`tracing::instrument`) in various places. For example:
```shell
JJ_TRACE=/tmp/trace.json jj diff
```
Then go to `https://ui.perfetto.dev/` in Chrome and load `/tmp/trace.json` from
there.
