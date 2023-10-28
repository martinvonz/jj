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
split up. Include tests and documentation in the same commit as the code the
test and document. The commit message should describe the changes in the commit;
the PR description can even be empty, but feel free to include a personal
message.

When you address comments on a PR, don't make the changes in a commit on top (as
is typical on GitHub). Instead, please make the changes in the appropriate
commit. You can do that by checking out the commit (`jj checkout/new <commit>`)
and then squash in the changes when you're done (`jj squash`). `jj git push`
will automatically force-push the branch.

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

You will probably also want to make the `gh-pages` branch immutable (and thereby
hidden from the default `jj log` output) by running the following in your repo:

```shell
jj config set --repo "revset-aliases.immutable_heads()" "main@origin | gh-pages@origin"
```

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


## Previewing the HTML documentation

The documentation for `jj` is automatically published to the website at
<https://martinvonz.github.io/jj/>.

When editing documentation, we'd appreciate it if you checked that the
result will look as expected when published to the website.

### Setting up the prerequisites

To build the website, you must have Python and `poetry` installed. If
your distribution packages `poetry`, something like `apt install
python3-poetry` is likely the best way to install it. Otherwise, you
can download Python from <https://python.org> or follow the [Python
installation instructions]. Finally, follow the [Poetry installation
instructions].

[Python installation instructions]: https://docs.python.org/3/using/index.html
[Poetry installation instructions]: https://python-poetry.org/docs/#installation 

Once you have `poetry` installed, you should ask it to install the rest
of the required tools into a virtual environment as follows:

```shell
# --no-root avoids a harmless error message starting with Poetry 1.7
poetry install --no-root
```

You may get requests to "unlock a keyring", [an error messages about failing to
do so](https://github.com/python-poetry/poetry/issues/1917), or, in the case of
Poetry 1.7, it may [simply hang
indefinitely](https://github.com/python-poetry/poetry/issues/8623). The
workaround is to either to unlock the keyring or to run the following, and then
to try `poetry install --no-root` again:

```shell
# For sh-compatible shells or recent versions of `fish`
export PYTHON_KEYRING_BACKEND=keyring.backends.fail.Keyring
```

### Building the HTML docs locally (with live reload)

The HTML docs are built with [MkDocs](https://github.com/mkdocs/mkdocs). After
following the above steps, you should be able to view the docs by running

```shell
# Note: this and all the commands below should be run from the root of
# the `jj` source tree.
poetry run -- mkdocs serve
```

and opening <http://127.0.0.1:8000> in your browser.

As you edit the `md` files, the website should be rebuilt and reloaded in your
browser automatically, unless build errors occur.

You should occasionally check the terminal from which you ran `mkdocs serve` for
any build errors or warnings. Warnings about `"GET /versions.json HTTP/1.1" code
404` are expected and harmless.

### How to build the entire website (not usually necessary)

The full `jj` website includes the documentation for several `jj` versions
(`prerelease`, latest release, and the older releases). The top-level
URL <https://martinvonz.github.io/jj> redirects to
<https://martinvonz.github.io/jj/latest>, which in turn redirects to
the docs for the last stable version.

The different versions of documentation are managed and deployed with
[`mike`](https://github.com/jimporter/mike), which can be run with
`poetry run -- mike`.

On a POSIX system or WSL, one way to build the entire website is as follows (on
Windows, you'll need to understand and adapt the shell script):

1. Check out `jj` as a co-located `jj + git` repository (`jj clone --colocate`),
cloned from your fork of `jj` (e.g. `jjfan.github.com/jj`). You can also use a
pure Git repo if you prefer.

2. Make sure `jjfan.github.com/jj` includes the `gh-pages` branch of the jj repo
and run `git fetch origin gh-pages`.

3. Go to the GitHub repository settings, enable GitHub Pages, and configure them
to use the `gh-pages` branch (this is usually the default).

4. Run the same `sh` script that is used in GitHub CI (details below):

    ```shell
    .github/scripts/docs-build-deploy 'https://jjfan.github.io/jj/'\
        prerelease main --push
    ```

    This should build the version of the docs from the current commit,
    deploy it as a new commit to the `gh-pages` branch,
    and push the `gh-pages` branch to the origin.

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

7. (Optional) When you are done, you may want to reset the `gh-branches` to the
same spot as it is in the upstream. If you configured the `upstream` remote,
this can be done with:

    ```shell
    # This will LOSE any changes you made to `gh-pages`
    jj git fetch --remote upstream
    jj branch set gh-pages -r gh-pages@upstream
    jj git push --remote origin --branch gh-pages
    ```

    If you want to preserve some of the changes you made, you can do `jj branch
    set my-changes -r gh-pages` BEFORE running the above commands.

#### Explanation of the `docs-build-deploy` script

The script sets up the `site_url` mkdocs config to
`'https://jjfan.github.io/jj/'`. If this config does not match the URL
where you loaded the website, some minor website features (like the
version switching widget) will have reduced functionality.

Then, the script passes the rest of its arguments to `potery run -- mike
deploy`, which does the rest of the job. Run `poetry run -- mike help deploy` to
find out what the arguments do.

If you need to do something more complicated, you can use `poetry run -- mike
...` commands. You can also edit the `gh-pages` branch directly, but take care
to avoid files that will be overwritten by future invocations of `mike`. Then,
you can submit a PR based on the `gh-pages` branch of
<https://martinvonz.github.com/jj> (instead of the usual `main` branch).


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
