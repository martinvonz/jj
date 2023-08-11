# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:artifact_tset.bzl", "ArtifactTSet")  # @unused Used as a type
load(
    "@prelude//linking:link_info.bzl",
    "LinkArgs",
    "SwiftmoduleLinkable",  # @unused Used as a type
)
load(
    "@prelude//linking:linkables.bzl",
    "LinkableProviders",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibrary",  # @unused Used as a type
)
load(":argsfiles.bzl", "CompileArgsfiles")
load(
    ":compile.bzl",
    "CxxSrcWithFlags",  # @unused Used as a type
)
load(
    ":headers.bzl",
    "CxxHeadersLayout",
)
load(
    ":link_groups.bzl",
    "LinkGroupInfo",  # @unused Used as a type
    "LinkGroupLibSpec",  # @unused Used as a type
)
load(
    ":linker.bzl",
    "SharedLibraryFlagOverrides",
)
load(
    ":preprocessor.bzl",
    "CPreprocessor",
    "CPreprocessorInfo",
)
load(
    ":xcode.bzl",
    "cxx_populate_xcode_attributes",
)

# Parameters to control which sub targets to define when processing Cxx rules.
# By default, generates all subtargets.
CxxRuleSubTargetParams = record(
    argsfiles = field(bool, True),
    compilation_database = field(bool, True),
    clang_remarks = field(bool, True),
    clang_traces = field(bool, True),
    headers = field(bool, True),
    link_group_map = field(bool, True),
    link_style_outputs = field(bool, True),
    xcode_data = field(bool, True),
    objects = field(bool, True),
    bitcode_bundle = field(bool, True),
)

# Parameters to control which providers to define when processing Cxx rules.
# By default, generates all providers.
CxxRuleProviderParams = record(
    compilation_database = field(bool, True),
    default = field(bool, True),
    java_packaging_info = field(bool, True),
    android_packageable_info = field(bool, True),
    linkable_graph = field(bool, True),
    link_style_outputs = field(bool, True),
    merged_native_link_info = field(bool, True),
    omnibus_root = field(bool, True),
    preprocessors = field(bool, True),
    resources = field(bool, True),
    shared_libraries = field(bool, True),
    template_placeholders = field(bool, True),
    preprocessor_for_tests = field(bool, True),
)

# Parameters to handle non-Clang sources, e.g Swift on Apple's platforms.
CxxRuleAdditionalParams = record(
    srcs = field([CxxSrcWithFlags.type], []),
    # Additional argsfiles to include for this rule.
    argsfiles = field(CompileArgsfiles.type, CompileArgsfiles()),
    # External debug info to be used when generated static output
    static_external_debug_info = field([ArtifactTSet.type], []),
    # External debug info to be used when generating shared objects
    shared_external_debug_info = field([ArtifactTSet.type], []),
    subtargets = field(dict, {}),  # [str: ["provider"]]
    # Might be used to expose additional providers to cxx layer (e.g to support #headers subtarget for Swift)
    additional_providers_factory = field(["function", None], None),  # (["CPreprocessorInfo", None]) -> ["provider"]:
)

# Parameters that allows to configure/extend generic implementation of C++ rules.
# Apple-specific rules (such as `apple_binary` and `apple_library`) and regular C++
# rules (such as `cxx_binary` and `cxx_library`) have too much in common, though
# some aspects of behavior (like layout of headers affecting inclusion of those
# or additional linking flags to support usage of platform frameworks) of are
# different and need to be specified. The following record holds the data which
# is needed to specialize user-facing rule from generic implementation.
CxxRuleConstructorParams = record(
    # Whether to build an empty shared library. This is utilized for rust_python_extensions
    # so that they can link against the rust shared object.
    build_empty_so = field(bool, False),
    # Name of the top level rule utilizing the cxx rule.
    rule_type = str,
    # If the rule is a test.
    is_test = field(bool, False),
    # Header layout to use importing headers.
    headers_layout = CxxHeadersLayout.type,
    # Additional information used to preprocess every unit of translation in the rule.
    extra_preprocessors = field([CPreprocessor.type], []),
    extra_preprocessors_info = field([CPreprocessorInfo.type], []),
    # Additional preprocessor info to export to other rules.
    extra_exported_preprocessors = field([CPreprocessor.type], []),
    # Additional information used to link every object produced by the rule,
    # flags are _both_ exported and used to link the target itself.
    extra_exported_link_flags = field([""], []),
    # Additional flags used _only_ when linking the target itself.
    # These flags are _not_ propagated up the dep tree.
    extra_link_flags = field([""], []),
    extra_binary_link_flags = field([""], []),
    # Additional artifacts to be linked together with the cxx compilation output.
    extra_link_input = field(["artifact"], []),
    # If True the extra_link_input should be considered as external debug info.
    extra_link_input_has_external_debug_info = field(bool, False),
    # Additional args to be used to link the target.
    extra_link_args = field([LinkArgs.type], []),
    # The swift module linkable that should be included for linking.
    swiftmodule_linkable = field([SwiftmoduleLinkable.type, None], None),
    # The source files to compile as part of this rule. This list can be generated
    # from ctx.attrs with the `get_srcs_with_flags` function.
    srcs = field([CxxSrcWithFlags.type]),
    additional = field(CxxRuleAdditionalParams.type, CxxRuleAdditionalParams()),
    # A function which enables the caller to inject subtargets into the link_style provider
    # as well as create custom providers based on the link styles.
    link_style_sub_targets_and_providers_factory = field("function", lambda _link_style, _context, _output: ({}, [])),
    # Linker flags that tell the linker to create shared libraries, overriding the default shared library flags.
    # e.g. when building Apple tests, we want to link with `-bundle` instead of `-shared` to allow
    # linking against the bundle loader.
    shared_library_flags = field([SharedLibraryFlagOverrides.type, None], None),
    # Optional argument to override the default name of the shared object being produced.
    soname = field([str, None], None),
    # If passed to cxx_executable, this field will be used to determine
    # a shared subtarget's default output should be stripped.
    strip_executable = field(bool, False),
    strip_args_factory = field("function", lambda _: cmd_args()),
    # Whether to embed the library name as the SONAME.
    use_soname = field(bool, True),
    # Use link group's linking logic regardless whether a link group map's present.
    force_link_group_linking = field(bool, False),
    # Function to use for setting Xcode attributes for the Xcode data sub target.
    cxx_populate_xcode_attributes_func = field("function", cxx_populate_xcode_attributes),
    # Define which sub targets to generate.
    generate_sub_targets = field(CxxRuleSubTargetParams.type, CxxRuleSubTargetParams()),
    # Define which providers to generate.
    generate_providers = field(CxxRuleProviderParams.type, CxxRuleProviderParams()),
    # Force this library to be a Python Omnibus root.
    is_omnibus_root = field(bool, False),
    # Emit an Omnibus shared root for this node even if it's not an Omnibus
    # root. This only makes sense to use in tests.
    force_emit_omnibus_shared_root = field(bool, False),
    force_full_hybrid_if_capable = field(bool, False),
    # Whether shared libs for executables should generate a shared lib link tree.
    exe_shared_libs_link_tree = field(bool, True),
    extra_link_deps = field([LinkableProviders.type], []),
    # Additional link roots (e.g. dlopen-able libs in a native python link),
    # other than the main executable, which can influence the C++ executable
    # result (e.g. added to link group linking or needs to be searched for
    # shared libs to include in the symlink tree).
    extra_link_roots = field([LinkableProviders.type], []),
    # Additional shared libs to "package".
    extra_shared_libs = field({str: SharedLibrary.type}, {}),
    auto_link_group_specs = field([[LinkGroupLibSpec.type], None], None),
    link_group_info = field([LinkGroupInfo.type, None], None),
    # Whether to use pre-stripped objects when linking.
    prefer_stripped_objects = field(bool, False),
    # The category suffix to use for executables actions (e.g. linking).
    exe_category_suffix = field(str, "executable"),
    # Whether link groups liking should make `preferred_linkage = "static"` libs
    # "follow" their dependents across link group boundaries.
    link_groups_force_static_follows_dependents = field(bool, True),
    # The intended return type is: (["_arglike"], {str: [DefaultInfo.type]}).
    extra_linker_outputs_factory = field("function", lambda _context: ([], {})),
)
