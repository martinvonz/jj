# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# @unsorted-dict-items
_rust_toolchain_attrs = {
    # Report unused dependencies
    "report_unused_deps": False,
    # Rustc target triple to use
    # https://doc.rust-lang.org/rustc/platform-support.html
    "rustc_target_triple": None,
    # Baseline compiler config
    "rustc_flags": [],
    # Extra flags when building binaries
    "rustc_binary_flags": [],
    # Extra flags for doing check builds
    "rustc_check_flags": [],
    # Extra flags for doing building tests
    "rustc_test_flags": [],
    # Extra flags when coverage is enabled for a target
    "rustc_coverage_flags": ["-Cinstrument-coverage"],
    # Extra flags for rustdoc invocations
    "rustdoc_flags": [],
    # Use rmeta for lib->lib dependencies, and only block
    # linking on rlib crates. The hope is that rmeta builds
    # are quick and this increases effective parallelism.
    "pipelined": False,
    # When you `buck test` a library, also compile and run example code in its
    # documentation comments.
    "doctests": False,
    # Filter out failures when we just need diagnostics. That is,
    # a rule which fails with a compilation failure will report
    # success as an RE action, but a "failure filter" action will
    # report the failure if some downstream action needs one of the
    # artifacts. If all you need is diagnostics, then it will report
    # success. This doubles the number of actions, so it should only
    # be explicitly enabled when needed.
    "failure_filter": False,
    # The Rust compiler (rustc)
    "compiler": None,
    # Rust documentation extractor (rustdoc)
    "rustdoc": None,
    # Clippy (linter) version of the compiler
    "clippy_driver": None,
    # Wrapper for rustc in actions
    "rustc_action": None,
    # Wrapper for rustdoc-generated test executables
    "rustdoc_test_with_resources": None,
    # Failure filter action
    "failure_filter_action": None,
    # The default edition to use, if not specified.
    "default_edition": None,
    # Lints
    "allow_lints": [],
    "deny_lints": [],
    "warn_lints": [],
    # Clippy configuration file clippy.toml
    "clippy_toml": None,
    # URL prefix (e.g. /path/to/docs) where crates' docs are hosted. Used for
    # linking types in signatures to their definition in another crate.
    "extern_html_root_url_prefix": None,
    # Utilities used for building flagfiles containing dynamic crate names
    "concat_tool": None,
    "transitive_dependency_symlinks_tool": None,
}

RustToolchainInfo = provider(fields = _rust_toolchain_attrs.keys())

def ctx_toolchain_info(ctx: AnalysisContext) -> RustToolchainInfo.type:
    toolchain_info = ctx.attrs._rust_toolchain[RustToolchainInfo]

    attrs = dict()
    for k, default in _rust_toolchain_attrs.items():
        v = getattr(toolchain_info, k)
        attrs[k] = default if v == None else v

    return RustToolchainInfo(**attrs)
