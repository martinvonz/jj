# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(
    "@prelude//:resources.bzl",
    "create_resource_db",
    "gather_resources",
)
load("@prelude//cxx:cxx_library_utility.bzl", "cxx_attr_deps")
load("@prelude//cxx:cxx_link_utility.bzl", "executable_shared_lib_arguments")
load("@prelude//cxx:linker.bzl", "PDB_SUB_TARGET")
load(
    "@prelude//linking:link_info.bzl",
    "LinkStyle",
    "Linkage",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "merge_shared_libraries",
    "traverse_shared_library_info",
)
load("@prelude//os_lookup:defs.bzl", "OsLookup")
load(
    "@prelude//tests:re_utils.bzl",
    "get_re_executor_from_props",
)
load("@prelude//utils:utils.bzl", "flatten_dict")
load("@prelude//test/inject_test_run_info.bzl", "inject_test_run_info")
load(
    ":build.bzl",
    "compile_context",
    "generate_rustdoc",
    "rust_compile",
    "rust_compile_multi",
)
load(
    ":build_params.bzl",
    "Emit",
    "LinkageLang",
    "RuleType",
    "build_params",
    "output_filename",
)
load(":context.bzl", "CompileContext")
load(
    ":link_info.bzl",
    "DEFAULT_STATIC_LINK_STYLE",
    "attr_simple_crate_for_filenames",
    "inherited_non_rust_shared_libs",
)
load(":resources.bzl", "rust_attr_resources")

def _rust_binary_common(
        ctx: AnalysisContext,
        compile_ctx: CompileContext.type,
        default_roots: list[str],
        extra_flags: list[str]) -> (list[[DefaultInfo.type, RunInfo.type]], cmd_args):
    toolchain_info = compile_ctx.toolchain_info

    simple_crate = attr_simple_crate_for_filenames(ctx)

    styles = {}
    dwp_target = None
    style_param = {}  # style -> param

    specified_link_style = LinkStyle(ctx.attrs.link_style) if ctx.attrs.link_style else DEFAULT_STATIC_LINK_STYLE

    target_os_type = ctx.attrs._target_os_type[OsLookup]
    linker_type = compile_ctx.cxx_toolchain_info.linker_info.type

    resources = flatten_dict(gather_resources(
        label = ctx.label,
        resources = rust_attr_resources(ctx),
        deps = cxx_attr_deps(ctx),
    ).values())

    for link_style in LinkStyle:
        params = build_params(
            rule = RuleType("binary"),
            proc_macro = False,
            link_style = link_style,
            preferred_linkage = Linkage("any"),
            lang = LinkageLang("rust"),
            linker_type = linker_type,
            target_os_type = target_os_type,
        )
        style_param[link_style] = params
        name = link_style.value + "/" + output_filename(simple_crate, Emit("link"), params)
        output = ctx.actions.declare_output(name)

        # Gather and setup symlink tree of transitive shared library deps.
        shared_libs = {}

        # As per v1, we only setup a shared library symlink tree for the shared
        # link style.
        # XXX need link tree for dylib crates
        if link_style == LinkStyle("shared"):
            shlib_info = merge_shared_libraries(
                ctx.actions,
                deps = inherited_non_rust_shared_libs(ctx),
            )
            for soname, shared_lib in traverse_shared_library_info(shlib_info).items():
                shared_libs[soname] = shared_lib.lib
        extra_link_args, runtime_files, _ = executable_shared_lib_arguments(
            ctx.actions,
            compile_ctx.cxx_toolchain_info,
            output,
            shared_libs,
        )

        extra_flags = toolchain_info.rustc_binary_flags + (extra_flags or [])

        # Compile rust binary.
        link, meta = rust_compile_multi(
            ctx = ctx,
            compile_ctx = compile_ctx,
            emits = [Emit("link"), Emit("metadata")],
            params = params,
            link_style = link_style,
            default_roots = default_roots,
            extra_link_args = extra_link_args,
            predeclared_outputs = {Emit("link"): output},
            extra_flags = extra_flags,
            is_binary = True,
        )

        args = cmd_args(link.output).hidden(runtime_files)
        extra_targets = [("check", meta.output)] + meta.diag.items()
        if link.pdb:
            extra_targets.append((PDB_SUB_TARGET, link.pdb))

        # If we have some resources, write it to the resources JSON file and add
        # it and all resources to "runtime_files" so that we make to materialize
        # them with the final binary.
        if resources:
            resources_hidden = [create_resource_db(
                ctx = ctx,
                name = name + ".resources.json",
                binary = output,
                resources = resources,
            )]
            for resource, other in resources.values():
                resources_hidden.append(resource)
                resources_hidden.extend(other)
            args.hidden(resources_hidden)
            runtime_files.extend(resources_hidden)

        styles[link_style] = (link.output, args, extra_targets, runtime_files)
        if link_style == specified_link_style and link.dwp_output:
            dwp_target = link.dwp_output

    expand = rust_compile(
        ctx = ctx,
        compile_ctx = compile_ctx,
        emit = Emit("expand"),
        params = style_param[DEFAULT_STATIC_LINK_STYLE],
        link_style = DEFAULT_STATIC_LINK_STYLE,
        default_roots = default_roots,
        extra_flags = extra_flags,
    )

    (link, args, extra_targets, runtime_files) = styles[specified_link_style]
    extra_targets += [
        ("doc", generate_rustdoc(
            ctx = ctx,
            compile_ctx = compile_ctx,
            params = style_param[DEFAULT_STATIC_LINK_STYLE],
            default_roots = default_roots,
            document_private_items = True,
        )),
        ("expand", expand.output),
        ("sources", compile_ctx.symlinked_srcs),
    ]
    sub_targets = {k: [DefaultInfo(default_output = v)] for k, v in extra_targets}
    for (k, (sub_link, sub_args, _sub_extra, sub_runtime_files)) in styles.items():
        sub_targets[k.value] = [
            DefaultInfo(
                default_output = sub_link,
                other_outputs = sub_runtime_files,
                # Check/save-analysis for each link style?
                # sub_targets = { k: [DefaultInfo(default_output = v)] for k, v in sub_extra }
            ),
            RunInfo(args = sub_args),
        ]

    if dwp_target:
        sub_targets["dwp"] = [
            DefaultInfo(
                default_output = dwp_target,
            ),
        ]

    providers = [
        DefaultInfo(
            default_output = link,
            other_outputs = runtime_files,
            sub_targets = sub_targets,
        ),
    ]
    return (providers, args)

def rust_binary_impl(ctx: AnalysisContext) -> list[[DefaultInfo.type, RunInfo.type]]:
    compile_ctx = compile_context(ctx)

    providers, args = _rust_binary_common(
        ctx = ctx,
        compile_ctx = compile_ctx,
        default_roots = ["main.rs"],
        extra_flags = [],
    )

    return providers + [RunInfo(args = args)]

def rust_test_impl(ctx: AnalysisContext) -> list[[DefaultInfo.type, RunInfo.type, ExternalRunnerTestInfo.type]]:
    compile_ctx = compile_context(ctx)
    toolchain_info = compile_ctx.toolchain_info

    extra_flags = toolchain_info.rustc_test_flags or []
    if ctx.attrs.framework:
        extra_flags += ["--test"]

    providers, args = _rust_binary_common(
        ctx = ctx,
        compile_ctx = compile_ctx,
        default_roots = ["main.rs", "lib.rs"],
        extra_flags = extra_flags,
    )

    # Setup a RE executor based on the `remote_execution` param.
    re_executor = get_re_executor_from_props(ctx.attrs.remote_execution)

    return inject_test_run_info(
        ctx,
        ExternalRunnerTestInfo(
            type = "rust",
            command = [args],
            env = ctx.attrs.env,
            labels = ctx.attrs.labels,
            contacts = ctx.attrs.contacts,
            default_executor = re_executor,
            # We implicitly make this test via the project root, instead of
            # the cell root (e.g. fbcode root).
            run_from_project_root = re_executor != None,
            use_project_relative_paths = re_executor != None,
        ),
    ) + providers
