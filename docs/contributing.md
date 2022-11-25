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

### Community Guidelines

This project follows [Google's Open Source Community
Guidelines](https://opensource.google/conduct/).

## Setting up a development environment

To develop `jj`, the mandatory steps are simply to [install
Rust](https://www.rust-lang.org/tools/install), clone the repository,
and use `cargo build`, `cargo fmt`, `cargo test`, etc.  If you are
preparing a PR, there are some additional recommended steps.

### Summary

One-time setup:

    rustup toolchain add nightly  # If this is not your default
    rustup toolchain add 1.60
    cargo install cargo-insta
    cargo install cargo-watch

During development (adapt according to your preference):

    cargo watch --ignore '.jj/**' -s 'cargo +nightly fmt && cargo clippy && cargo +1.60 check'
    cargo insta test  # Occasionally

WARNING: Build artifacts from debug builds and especially from repeated
invocations of `cargo test` can quickly take up 10s of GB of disk space.
Cargo will happily use up your entire hard drive. If this happens, run
`cargo clean`.

### Explanation

These are ordered roughly in order of decreasing importance.

1. Nearly any change to `jj`'s interface will require writing or updating
golden tests that use the [`insta`](https://insta.rs/) crate.  To make
this convenient, install the `cargo-insta` binary. Use `cargo insta tests`
to run tests, and `cargo insta review` to update the golden tests.

2. Github CI checks require that the code is formatted with the *nightly*
version of `rustfmt`. To do this on your computer, install the nightly
toolchain and use `cargo +nightly fmt`.

3. Your code will be rejected if it cannot be compiled with the
minimal supported version of Rust. This version is listed as
`rust-version` in [`Cargo.toml`](../Cargo.toml); it is 1.60 as
of this writing.

4. Your code needs to pass `cargo clippy`. You can also use `cargo
+nightly clippy` if you wish to see more warnings.

5. You may also want to install and use `cargo-watch`. In this case,
you should exclude `.jj`. directory from the filesystem watcher, as
it gets updated on every `jj log`.
