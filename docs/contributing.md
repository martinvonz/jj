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
separately, and we don't squash them when the PR is ready.

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


### Community Guidelines

This project follows [Google's Open Source Community
Guidelines](https://opensource.google/conduct/).


## Learning Rust

In addition to the [Rust Book](https://doc.rust-lang.org/book/) and the other
excellent resources at https://www.rust-lang.org/learn, we recommend the
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

If you already have `rustup` installed, then moving into the source code
directory will install the MSRV for you.

One-time setup:

    rustup toolchain add nightly  # wanted for 'rustfmt'
    cargo install cargo-insta
    cargo install cargo-watch
    cargo install cargo-nextest

During development (adapt according to your preference):

    cargo watch --ignore '.jj/**' -s \
      'cargo clippy --workspace --all-targets \
       && cargo check --workspace --all-targets'
    cargo +nightly fmt # Occasionally
    cargo nextest run --workspace # Occasionally
    cargo insta test --workspace # Occasionally

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

2. Github CI checks require that the code is formatted with the *nightly*
   version of `rustfmt`. To do this on your computer, install the nightly
   toolchain and use `cargo +nightly fmt`.

3. Your code will be rejected if it cannot be compiled with the minimal
   supported version of Rust ("MSRV"). Currently, `jj` follows a rather
   casual MSRV policy: "The current `rustc` stable version, minus one."

4. Your code needs to pass `cargo clippy`. You can also
   use `cargo +nightly clippy` if you wish to see more warnings.

5. You may also want to install and use `cargo-watch`. In this case, you should
   exclude `.jj`. directory from the filesystem watcher, as it gets updated on
   every `jj log`.

6. To run tests more quickly, use `cargo nextest run --workspace`. To
   use `nextest` with `insta`,
   use `cargo insta test --workspace --test-runner nextest`.
 
 ## Modifying protobuffers (this is not common)
 
 Occasionally, you may need to change the `.proto` files that define jj's data
 storage format. In this case, you will need to add a few steps to the above
 workflow.
 
 - Install the `protoc` compiler. This usually means either `apt-get install
   protobuf-compiler` or downloading [an official release]. The 
   [`prost` library docs] have additional advice.
 - Run `cargo run -p gen-protos` regularly (or after every edit to a `.proto`
   file). This is the same as running `cargo run` from `lib/gen-protos`. The
   `gen-protos` binary will use the `prost-build` library to compile the
   `.proto` files into `.rs` files.
 - If you are adding a new `.proto` file, you will need to edit the list of
   these files in `lib/gen-protos/src/main.rs`.

[an official release]: https://github.com/protocolbuffers/protobuf/releases
[`prost` library docs]: https://docs.rs/prost-build/latest/prost_build/#sourcing-protoc

 The `.rs` files generated from `.proto` files are included in the repository,
 and there is a Github CI check that will complain if they do not match.
