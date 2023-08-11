# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//cxx:omnibus.bzl", "omnibus_environment_attr")
load("@prelude//cxx/user:link_group_map.bzl", "link_group_map_attr")
load("@prelude//rust:rust_binary.bzl", "rust_binary_impl", "rust_test_impl")
load("@prelude//rust:rust_library.bzl", "prebuilt_rust_library_impl", "rust_library_impl")
load(":common.bzl", "LinkableDepType", "Linkage", "buck", "prelude_rule")
load(":native_common.bzl", "native_common")
load(":rust_common.bzl", "rust_common")

prebuilt_rust_library = prelude_rule(
    name = "prebuilt_rust_library",
    impl = prebuilt_rust_library_impl,
    docs = """
        A prebuilt\\_rust\\_library() specifies a pre-built Rust crate, and any dependencies
        it may have on other crates (typically also prebuilt).


        Note: Buck is currently tested with (and therefore supports) version 1.32.0 of Rust.
    """,
    examples = """
        ```

        prebuilt_rust_library(
          name = 'dailygreet',
          rlib = 'libdailygreet.rlib',
          deps = [
            ':jinsy',
          ],
        )

        prebuilt_rust_library(
          name = 'jinsy',
          rlib = 'libarbiter-6337e9cb899bd295.rlib',
        )

        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "rlib": attrs.source(doc = """
                Path to the precompiled Rust crate - typically of the form 'libfoo.rlib', or
                'libfoo-abc123def456.rlib' if it has symbol versioning metadata.
            """),
        } |
        rust_common.crate(crate_type = attrs.string(default = "")) |
        rust_common.deps_arg() |
        {
            "contacts": attrs.list(attrs.string(), default = []),
            "default_host_platform": attrs.option(attrs.configuration_label(), default = None),
            "labels": attrs.list(attrs.string(), default = []),
            "licenses": attrs.list(attrs.source(), default = []),
            "link_style": attrs.option(attrs.enum(LinkableDepType), default = None),
            "proc_macro": attrs.bool(default = False),
        } |
        rust_common.toolchains_args()
    ),
)

_RUST_COMMON_ATTRIBUTES = {
    "contacts": attrs.list(attrs.string(), default = []),
    "coverage": attrs.bool(default = False),
    "default_host_platform": attrs.option(attrs.configuration_label(), default = None),
    "default_platform": attrs.option(attrs.string(), default = None),
    "flagged_deps": attrs.list(attrs.tuple(attrs.dep(), attrs.list(attrs.string())), default = []),
    "incremental_build_mode": attrs.option(attrs.string(), default = None),
    "incremental_enabled": attrs.bool(default = False),
    "labels": attrs.list(attrs.string(), default = []),
    "licenses": attrs.list(attrs.source(), default = []),
    # linker_flags weren't supported for rust_library in Buck v1 but the
    # fbcode macros pass them anyway. They're typically empty since the
    # config-level flags don't get injected, but it doesn't hurt to accept
    # them and it simplifies the implementation of Rust rules since they
    # don't have to know whether we're building a rust_binary or a
    # rust_library.
    "linker_flags": attrs.list(attrs.arg(anon_target_compatible = True), default = []),
    "resources": attrs.named_set(attrs.one_of(attrs.dep(), attrs.source()), sorted = True, default = []),
    "rustdoc_flags": attrs.list(attrs.arg(), default = []),
    "version_universe": attrs.option(attrs.string(), default = None),
    "_exec_os_type": buck.exec_os_type_arg(),
    "_target_os_type": buck.target_os_type_arg(),
}

_RUST_EXECUTABLE_ATTRIBUTES = {
    # TODO: enable distributed thinlto
    "enable_distributed_thinlto": attrs.bool(default = False),
    # TODO: enable link groups
    "link_group": attrs.option(attrs.string(), default = None),
    "link_group_map": link_group_map_attr(),
    "link_group_min_binary_node_count": attrs.option(attrs.int(), default = None),
    "rpath": attrs.bool(default = False, doc = """
              Set the "rpath" in the executable when using a shared link style.
          """),
}

rust_binary = prelude_rule(
    name = "rust_binary",
    impl = rust_binary_impl,
    docs = """
        A rust\\_binary() rule builds a native executable from the supplied set of Rust source files
        and dependencies.


        If you invoke a build with the `check` flavor, then Buck will invoke rustc
        to check the code (typecheck, produce warnings, etc), but won't generate an executable code.
        When applied to binaries it produces no output; for libraries it produces metadata for
        consumers of the library. When building with `check`, extra compiler flags from
        the `rust.rustc_check_flags` are added to the compiler's command line options,
        to allow for extra warnings, etc.


        Note: Buck is currently tested with (and therefore supports) version 1.32.0 of Rust.
    """,
    examples = """
        For more examples, check out our [integration tests](https://github.com/facebook/buck/tree/dev/test/com/facebook/buck/rust/testdata/).


        ```

        rust_binary(
          name='greet',
          srcs=[
            'greet.rs',
          ],
          deps=[
            ':greeting',
          ],
        )

        rust_library(
          name='greeting',
          srcs=[
            'greeting.rs',
          ],
          deps=[
            ':join',
          ],
        )

        rust_library(
          name='join',
          srcs=[
            'join.rs',
          ],
        )

        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        rust_common.srcs_arg() |
        rust_common.mapped_srcs_arg() |
        rust_common.deps_arg() |
        rust_common.named_deps_arg() |
        rust_common.edition_arg() |
        rust_common.features_arg() |
        rust_common.rustc_flags_arg() |
        rust_common.linker_flags_arg() |
        rust_common.env_arg() |
        rust_common.crate(crate_type = attrs.option(attrs.string(), default = None)) |
        rust_common.crate_root() |
        native_common.link_style() |
        _RUST_COMMON_ATTRIBUTES |
        _RUST_EXECUTABLE_ATTRIBUTES | {
            "framework": attrs.bool(default = False),
        } |
        rust_common.toolchains_args()
    ),
)

rust_library = prelude_rule(
    name = "rust_library",
    impl = rust_library_impl,
    docs = """
        A rust\\_library() rule builds a native library from the supplied set of Rust source files
        and dependencies.


        If you invoke a build with the `check` flavor, then Buck will invoke rustc
        to check the code (typecheck, produce warnings, etc), but won't generate an executable code.
        When applied to binaries it produces no output; for libraries it produces metadata for
        consumers of the library. When building with `check`, extra compiler flags from
        the `rust.rustc_check_flags` are added to the compiler's command line options,
        to allow for extra warnings, etc.


        Note: Buck is currently tested with (and therefore supports) version 1.32.0 of Rust.
    """,
    examples = """
        For more examples, check out our [integration tests](https://github.com/facebook/buck/tree/dev/test/com/facebook/buck/rust/testdata/).


        ```

        rust_library(
          name='greeting',
          srcs=[
            'greeting.rs',
          ],
          deps=[
            ':join',
          ],
        )

        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        rust_common.srcs_arg() |
        rust_common.mapped_srcs_arg() |
        rust_common.deps_arg() |
        rust_common.named_deps_arg() |
        rust_common.edition_arg() |
        rust_common.features_arg() |
        rust_common.rustc_flags_arg() |
        rust_common.env_arg() |
        rust_common.crate(crate_type = attrs.option(attrs.string(), default = None)) |
        rust_common.crate_root() |
        native_common.preferred_linkage(preferred_linkage_type = attrs.enum(Linkage, default = "any")) |
        _RUST_COMMON_ATTRIBUTES |
        {
            "crate_dynamic": attrs.option(attrs.dep(), default = None),
            "doc_deps": attrs.list(attrs.dep(), default = []),
            "doc_env": attrs.dict(key = attrs.string(), value = attrs.option(attrs.arg()), sorted = False, default = {}),
            "doc_linker_flags": attrs.list(attrs.arg(), default = []),
            "doc_named_deps": attrs.dict(key = attrs.string(), value = attrs.dep(), sorted = False, default = {}),
            "doctests": attrs.option(attrs.bool(), default = None),
            "doctest_link_style": attrs.option(attrs.enum(LinkableDepType), default = None, doc = """
            Like `link_style` on binaries, but applies specifically to doctests.
            """),
            "proc_macro": attrs.bool(default = False),
            "supports_python_dlopen": attrs.option(attrs.bool(), default = None),
            "_omnibus_environment": omnibus_environment_attr(),
        } |
        rust_common.toolchains_args()
    ),
)

rust_test = prelude_rule(
    name = "rust_test",
    impl = rust_test_impl,
    docs = """
        A rust\\_test() rule builds a Rust test native executable from the supplied set of Rust source
        files and dependencies and runs this test.


        Note: Buck is currently tested with (and therefore supports) version 1.32.0 of Rust.
    """,
    examples = """
        For more examples, check out our [integration tests](https://github.com/facebook/buck/tree/dev/test/com/facebook/buck/rust/testdata/).


        ```

        rust_test(
          name='greet',
          srcs=[
            'greet.rs',
          ],
          deps=[
            ':greeting',
          ],
        )

        rust_library(
          name='greeting',
          srcs=[
            'greeting.rs',
          ],
          deps=[
            ':join',
          ],
        )

        rust_library(
          name='join',
          srcs=[
            'join.rs',
          ],
        )

        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "framework": attrs.bool(default = False, doc = """
                Use the standard test framework. If this is set to false, then the result is a normal
                executable which requires a `main()`, etc. It is still expected to accept the
                same command-line parameters and produce the same output as the test framework.
            """),
        } |
        rust_common.srcs_arg() |
        rust_common.mapped_srcs_arg() |
        rust_common.deps_arg() |
        rust_common.named_deps_arg() |
        rust_common.edition_arg() |
        rust_common.features_arg() |
        rust_common.rustc_flags_arg() |
        rust_common.env_arg() |
        rust_common.crate(crate_type = attrs.option(attrs.string(), default = None)) |
        rust_common.crate_root() |
        native_common.link_style() |
        _RUST_COMMON_ATTRIBUTES |
        _RUST_EXECUTABLE_ATTRIBUTES |
        {
            "framework": attrs.bool(default = True),
            "remote_execution": buck.re_opts_for_tests_arg(),
        } |
        rust_common.toolchains_args()
    ),
)

rust_rules = struct(
    prebuilt_rust_library = prebuilt_rust_library,
    rust_binary = rust_binary,
    rust_library = rust_library,
    rust_test = rust_test,
)
