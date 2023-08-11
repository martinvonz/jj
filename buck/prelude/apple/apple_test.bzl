# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//apple:apple_library.bzl", "AppleLibraryAdditionalParams", "apple_library_rule_constructor_params_and_swift_providers")
load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolchainInfo")
# @oss-disable: load("@prelude//apple/meta_only:apple_test_re_capabilities.bzl", "apple_test_re_capabilities") 
# @oss-disable: load("@prelude//apple/meta_only:apple_test_re_use_case.bzl", "apple_test_re_use_case") 
load("@prelude//apple/swift:swift_compilation.bzl", "get_swift_anonymous_targets", "uses_explicit_modules")
load(
    "@prelude//cxx:argsfiles.bzl",
    "CompileArgsfile",  # @unused Used as a type
)
load(
    "@prelude//cxx:compile.bzl",
    "CxxSrcWithFlags",  # @unused Used as a type
)
load("@prelude//cxx:cxx_library.bzl", "cxx_library_parameterized")
load("@prelude//cxx:cxx_types.bzl", "CxxRuleProviderParams", "CxxRuleSubTargetParams")
load(
    "@prelude//cxx:linker.bzl",
    "SharedLibraryFlagOverrides",
)
load(
    "@prelude//utils:dicts.bzl",
    "flatten_x",
)
load(":apple_bundle.bzl", "AppleBundlePartListConstructorParams", "get_apple_bundle_part_list")
load(":apple_bundle_destination.bzl", "AppleBundleDestination")
load(":apple_bundle_part.bzl", "AppleBundlePart", "assemble_bundle", "bundle_output", "get_bundle_dir_name")
load(":apple_bundle_types.bzl", "AppleBundleInfo")
load(":apple_bundle_utility.bzl", "get_product_name")
load(":apple_dsym.bzl", "DSYM_SUBTARGET", "DWARF_AND_DSYM_SUBTARGET", "get_apple_dsym")
load(":apple_sdk.bzl", "get_apple_sdk_name")
load(
    ":apple_sdk_metadata.bzl",
    "MacOSXSdkMetadata",
)
load(":debug.bzl", "DEBUGINFO_SUBTARGET")
load(":xcode.bzl", "apple_populate_xcode_attributes")

def apple_test_impl(ctx: AnalysisContext) -> [list["provider"], "promise"]:
    def get_apple_test_providers(deps_providers) -> list["provider"]:
        xctest_bundle = bundle_output(ctx)

        test_host_app_bundle = _get_test_host_app_bundle(ctx)
        test_host_app_binary = _get_test_host_app_binary(ctx, test_host_app_bundle)

        objc_bridging_header_flags = [
            # Disable bridging header -> PCH compilation to mitigate an issue in Xcode 13 beta.
            "-disable-bridging-pch",
            "-import-objc-header",
            cmd_args(ctx.attrs.bridging_header),
        ] if ctx.attrs.bridging_header else []

        constructor_params = apple_library_rule_constructor_params_and_swift_providers(
            ctx,
            AppleLibraryAdditionalParams(
                rule_type = "apple_test",
                extra_exported_link_flags = _get_xctest_framework_linker_flags(ctx) + _get_bundle_loader_flags(test_host_app_binary),
                extra_swift_compiler_flags = _get_xctest_framework_search_paths_flags(ctx) + objc_bridging_header_flags,
                shared_library_flags = SharedLibraryFlagOverrides(
                    # When `-bundle` is used we can't use the `-install_name` args, thus we keep this field empty.
                    shared_library_name_linker_flags_format = [],
                    # When building Apple tests, we want to link with `-bundle` instead of `-shared` to allow
                    # linking against the bundle loader.
                    shared_library_flags = ["-bundle"],
                ),
                generate_sub_targets = CxxRuleSubTargetParams(
                    compilation_database = True,
                    headers = False,
                    link_group_map = False,
                ),
                generate_providers = CxxRuleProviderParams(
                    compilation_database = True,
                    default = False,
                    linkable_graph = False,
                    link_style_outputs = False,
                    merged_native_link_info = False,
                    omnibus_root = False,
                    preprocessors = False,
                    resources = False,
                    shared_libraries = False,
                    template_placeholders = False,
                ),
                populate_xcode_attributes_func = lambda local_ctx, **kwargs: _xcode_populate_attributes(ctx = local_ctx, xctest_bundle = xctest_bundle, test_host_app_binary = test_host_app_binary, **kwargs),
                # We want to statically link the transitive dep graph of the apple_test()
                # which we can achieve by forcing link group linking with
                # an empty mapping (i.e., default mapping).
                force_link_group_linking = True,
            ),
            deps_providers,
        )

        cxx_library_output = cxx_library_parameterized(ctx, constructor_params)
        test_binary_output = ctx.actions.declare_output(get_product_name(ctx))

        # Rename in order to generate dSYM with correct binary name (dsymutil doesn't provide a way to control binary name in output dSYM bundle).
        test_binary = ctx.actions.copy_file(test_binary_output, cxx_library_output.default_output.default)

        binary_part = AppleBundlePart(source = test_binary, destination = AppleBundleDestination("executables"), new_name = ctx.attrs.name)
        part_list_output = get_apple_bundle_part_list(ctx, AppleBundlePartListConstructorParams(binaries = [binary_part]))
        sub_targets = assemble_bundle(ctx, xctest_bundle, part_list_output.parts, part_list_output.info_plist_part, None)

        sub_targets.update(cxx_library_output.sub_targets)
        (debuginfo,) = sub_targets[DEBUGINFO_SUBTARGET]
        dsym_artifact = get_apple_dsym(
            ctx = ctx,
            executable = test_binary,
            debug_info = debuginfo.other_outputs,
            action_identifier = "generate_apple_test_dsym",
            output_path_override = get_bundle_dir_name(ctx) + ".dSYM",
        )
        sub_targets[DSYM_SUBTARGET] = [DefaultInfo(default_output = dsym_artifact)]

        # If the test has a test host, add a subtarget to build the test host app bundle.
        sub_targets["test-host"] = [DefaultInfo(default_output = test_host_app_bundle)] if test_host_app_bundle else [DefaultInfo()]

        sub_targets[DWARF_AND_DSYM_SUBTARGET] = [
            DefaultInfo(default_output = xctest_bundle, other_outputs = [dsym_artifact]),
            _get_test_info(ctx, xctest_bundle, test_host_app_bundle, dsym_artifact),
        ]

        return [
            DefaultInfo(default_output = xctest_bundle, sub_targets = sub_targets),
            _get_test_info(ctx, xctest_bundle, test_host_app_bundle),
            cxx_library_output.xcode_data_info,
            cxx_library_output.cxx_compilationdb_info,
        ]

    if uses_explicit_modules(ctx):
        return get_swift_anonymous_targets(ctx, get_apple_test_providers)
    else:
        return get_apple_test_providers([])

def _get_test_info(ctx: AnalysisContext, xctest_bundle: "artifact", test_host_app_bundle: ["artifact", None], dsym_artifact: ["artifact", None] = None) -> "provider":
    # When interacting with Tpx, we just pass our various inputs via env vars,
    # since Tpx basiclaly wants structured output for this.

    xctest_bundle = cmd_args(xctest_bundle).hidden(dsym_artifact) if dsym_artifact else xctest_bundle
    env = {"XCTEST_BUNDLE": xctest_bundle}

    if test_host_app_bundle == None:
        tpx_label = "tpx:apple_test:buck2:logicTest"
    else:
        env["HOST_APP_BUNDLE"] = test_host_app_bundle
        tpx_label = "tpx:apple_test:buck2:appTest"

    labels = ctx.attrs.labels + [tpx_label]
    labels.append(tpx_label)

    sdk_name = get_apple_sdk_name(ctx)
    if sdk_name == MacOSXSdkMetadata.name:
        # It's not possible to execute macOS tests on RE yet
        local_enabled = True
        remote_enabled = False
        remote_execution_properties = None
        remote_execution_use_case = None
    else:
        local_enabled = False
        remote_enabled = True
        # @oss-disable: requires_ios_booted_simulator = ctx.attrs.test_host_app != None or ctx.attrs.ui_test_target_app != None 
        # @oss-disable: remote_execution_properties = apple_test_re_capabilities(use_unbooted_simulator = not requires_ios_booted_simulator) 
        # @oss-disable: remote_execution_use_case = apple_test_re_use_case() 
        remote_execution_properties = None # @oss-enable
        remote_execution_use_case = None # @oss-enable

    return ExternalRunnerTestInfo(
        type = "custom",  # We inherit a label via the macro layer that overrides this.
        command = ["false"],  # Tpx makes up its own args, we just pass params via the env.
        env = flatten_x([ctx.attrs.env or {}, env]),
        labels = labels,
        use_project_relative_paths = True,
        run_from_project_root = True,
        contacts = ctx.attrs.contacts,
        executor_overrides = {
            "ios-simulator": CommandExecutorConfig(
                local_enabled = local_enabled,
                remote_enabled = remote_enabled,
                remote_execution_properties = remote_execution_properties,
                remote_execution_use_case = remote_execution_use_case,
            ),
            "static-listing": CommandExecutorConfig(local_enabled = True, remote_enabled = False),
        },
        local_resources = {
            "ios_booted_simulator": ctx.attrs._ios_booted_simulator.label,
            "macos_idb_companion": ctx.attrs._macos_idb_companion.label,
        },
    )

def _get_test_host_app_bundle(ctx: AnalysisContext) -> ["artifact", None]:
    """ Get the bundle for the test host app, if one exists for this test. """
    if ctx.attrs.test_host_app:
        # Copy the test host app bundle into test's output directory
        original_bundle = ctx.attrs.test_host_app[AppleBundleInfo].bundle
        test_host_app_bundle = ctx.actions.declare_output(original_bundle.basename)
        ctx.actions.copy_file(test_host_app_bundle, original_bundle)
        return test_host_app_bundle

    return None

def _get_test_host_app_binary(ctx: AnalysisContext, test_host_app_bundle: ["artifact", None]) -> [cmd_args, None]:
    """ Reference to the binary with the test host app bundle, if one exists for this test. Captures the bundle as an artifact in the cmd_args. """
    if ctx.attrs.test_host_app:
        return cmd_args([test_host_app_bundle, ctx.attrs.test_host_app[AppleBundleInfo].binary_name], delimiter = "/")

    return None

def _get_bundle_loader_flags(binary: [cmd_args, None]) -> list[""]:
    if binary:
        # During linking we need to link the test shared lib against the test host binary. The
        # test host binary doesn't need to be embedded in an `apple_bundle`.
        return ["-bundle_loader", binary]

    return []

def _xcode_populate_attributes(
        ctx: AnalysisContext,
        srcs: list[CxxSrcWithFlags.type],
        argsfiles: dict[str, CompileArgsfile.type],
        xctest_bundle: "artifact",
        test_host_app_binary: [cmd_args, None],
        **_kwargs) -> dict[str, ""]:
    data = apple_populate_xcode_attributes(ctx = ctx, srcs = srcs, argsfiles = argsfiles, product_name = ctx.attrs.name)
    data["output"] = xctest_bundle
    if test_host_app_binary:
        data["test_host_app_binary"] = test_host_app_binary
    return data

def _get_xctest_framework_search_paths(ctx: AnalysisContext) -> (cmd_args, cmd_args):
    toolchain = ctx.attrs._apple_toolchain[AppleToolchainInfo]
    xctest_swiftmodule_search_path = cmd_args([toolchain.platform_path, "Developer/usr/lib"], delimiter = "/")
    xctest_framework_search_path = cmd_args([toolchain.platform_path, "Developer/Library/Frameworks"], delimiter = "/")
    return (xctest_swiftmodule_search_path, xctest_framework_search_path)

def _get_xctest_framework_search_paths_flags(ctx: AnalysisContext) -> list[[cmd_args, str]]:
    xctest_swiftmodule_search_path, xctest_framework_search_path = _get_xctest_framework_search_paths(ctx)
    return [
        "-I",
        xctest_swiftmodule_search_path,
        "-F",
        xctest_framework_search_path,
    ]

def _get_xctest_framework_linker_flags(ctx: AnalysisContext) -> list[[cmd_args, str]]:
    xctest_swiftmodule_search_path, xctest_framework_search_path = _get_xctest_framework_search_paths(ctx)
    return [
        "-L",
        xctest_swiftmodule_search_path,
        "-F",
        xctest_framework_search_path,
    ]
