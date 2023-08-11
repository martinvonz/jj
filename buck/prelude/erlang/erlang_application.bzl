# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:paths.bzl", "paths")
load(":erlang_build.bzl", "erlang_build")
load(
    ":erlang_dependencies.bzl",
    "ErlAppDependencies",
    "check_dependencies",
    "flatten_dependencies",
)
load(
    ":erlang_info.bzl",
    "ErlangAppIncludeInfo",
    "ErlangAppInfo",
)
load(":erlang_shell.bzl", "erlang_shell")
load(
    ":erlang_toolchain.bzl",
    "get_primary",
    "select_toolchains",
)
load(
    ":erlang_utils.bzl",
    "action_identifier",
    "build_paths",
    "convert",
    "multidict_projection",
    "multidict_projection_key",
    "normalise_metadata",
    "str_to_bool",
    "to_term_args",
)

StartDependencySet = transitive_set()

StartTypeValues = ["permanent", "transient", "temporary", "load", "none"]

StartType = enum(*StartTypeValues)

StartSpec = record(
    name = field("string"),
    version = field("string"),
    resolved = field("bool"),
    start_type = field("StartType"),
)

def erlang_application_impl(ctx: AnalysisContext) -> list["provider"]:
    # select the correct tools from the toolchain
    toolchains = select_toolchains(ctx)

    # collect all dependencies
    all_direct_dependencies = (check_dependencies(ctx.attrs.applications, [ErlangAppInfo]) +
                               check_dependencies(ctx.attrs.included_applications, [ErlangAppInfo]) +
                               check_dependencies(ctx.attrs.extra_includes, [ErlangAppIncludeInfo]))
    dependencies = flatten_dependencies(ctx, all_direct_dependencies)

    return build_application(ctx, toolchains, dependencies, _build_erlang_application)

def build_application(ctx, toolchains, dependencies, build_fun) -> list["provider"]:
    name = ctx.attrs.name

    build_environments = {}
    app_folders = {}
    start_dependencies = {}
    for toolchain in toolchains.values():
        build_environment = build_fun(ctx, toolchain, dependencies)
        build_environments[toolchain.name] = build_environment

        # link final output
        app_folders[toolchain.name] = link_output(
            ctx,
            paths.join(
                erlang_build.utils.build_dir(toolchain),
                "linked",
                name,
            ),
            build_environment,
        )

        # build start dependencies in reverse order
        start_dependencies[toolchain.name] = _build_start_dependencies(ctx, toolchain)

    primary_build = build_environments[get_primary(ctx)]
    primary_app_folder = link_output(
        ctx,
        name,
        primary_build,
    )

    app_info = build_app_info(
        ctx,
        dependencies,
        build_environments,
        app_folders,
        primary_app_folder,
        start_dependencies,
    )

    # generate DefaultInfo and RunInfo providers
    default_info = _build_default_info(dependencies, primary_app_folder)
    run_info = erlang_shell.build_run_info(ctx, dependencies.values(), additional_app_paths = [primary_app_folder])
    return [
        default_info,
        run_info,
        app_info,
    ]

def _build_erlang_application(ctx: AnalysisContext, toolchain: "Toolchain", dependencies: ErlAppDependencies) -> "BuildEnvironment":
    name = ctx.attrs.name

    build_environment = erlang_build.prepare_build_environment(ctx, toolchain, dependencies)

    # build generated inputs
    generated_source_artifacts = erlang_build.build_steps.generated_source_artifacts(ctx, toolchain, name)

    # collect all inputs
    src_artifacts = [
        src
        for src in ctx.attrs.srcs
        if erlang_build.utils.is_erl(src) and erlang_build.utils.module_name(src) not in generated_source_artifacts
    ] + generated_source_artifacts.values()

    header_artifacts = ctx.attrs.includes

    private_header_artifacts = [header for header in ctx.attrs.srcs if erlang_build.utils.is_hrl(header)]

    # build input mapping
    build_environment = erlang_build.build_steps.generate_input_mapping(
        build_environment,
        src_artifacts + header_artifacts + private_header_artifacts,
    )

    # build output artifacts

    # public includes
    build_environment = erlang_build.build_steps.generate_include_artifacts(
        ctx,
        toolchain,
        build_environment,
        name,
        header_artifacts,
    )

    # private includes
    build_environment = erlang_build.build_steps.generate_include_artifacts(
        ctx,
        toolchain,
        build_environment,
        erlang_build.utils.private_include_name(toolchain, name),
        private_header_artifacts,
        is_private = True,
    )

    # beams
    build_environment = erlang_build.build_steps.generate_beam_artifacts(
        ctx,
        toolchain,
        build_environment,
        name,
        src_artifacts,
    )

    # edoc chunks (only materialised in edoc subtarget)
    build_environment = erlang_build.build_steps.generate_chunk_artifacts(
        ctx,
        toolchain,
        build_environment,
        name,
        src_artifacts,
    )

    # create <appname>.app file
    build_environment = _generate_app_file(
        ctx,
        toolchain,
        build_environment,
        name,
        src_artifacts,
    )

    # priv
    build_environment = _generate_priv_dir(
        ctx,
        toolchain,
        build_environment,
    )

    return build_environment

def _generate_priv_dir(
        ctx: AnalysisContext,
        toolchain: "Toolchain",
        build_environment: "BuildEnvironment") -> "BuildEnvironment":
    """Generate the application's priv dir."""
    name = ctx.attrs.name

    resources = ctx.attrs.resources
    priv_symlinks = {}
    for resource in resources:
        for file in resource[DefaultInfo].default_outputs:
            priv_symlinks[file.short_path] = file
        for file in resource[DefaultInfo].other_outputs:
            priv_symlinks[file.short_path] = file

    build_environment.priv_dirs[name] = ctx.actions.symlinked_dir(
        paths.join(
            erlang_build.utils.build_dir(toolchain),
            name,
            "priv",
        ),
        priv_symlinks,
    )
    return build_environment

def _generate_app_file(
        ctx: AnalysisContext,
        toolchain: "Toolchain",
        build_environment: "BuildEnvironment",
        name: str,
        srcs: list["artifact"]) -> "BuildEnvironment":
    """ rule for generating the .app files

    NOTE: We are using the .erl files as input to avoid dependencies on
          beams.
    """
    tools = toolchain.otp_binaries

    app_file_name = build_paths.app_file(ctx)
    output = ctx.actions.declare_output(
        paths.join(
            erlang_build.utils.build_dir(toolchain),
            app_file_name,
        ),
    )
    script = toolchain.app_file_script
    app_info_file = _app_info_content(ctx, toolchain, name, srcs, output)
    app_build_cmd = cmd_args(
        [
            tools.escript,
            script,
            app_info_file,
        ],
    )
    app_build_cmd.hidden(output.as_output())
    app_build_cmd.hidden(srcs)
    if ctx.attrs.app_src:
        app_build_cmd.hidden(ctx.attrs.app_src)
    erlang_build.utils.run_with_env(
        ctx,
        toolchain,
        app_build_cmd,
        category = "app_resource",
        identifier = action_identifier(toolchain, paths.basename(app_file_name)),
    )

    build_environment.app_files[name] = output

    return build_environment

def _app_info_content(
        ctx: AnalysisContext,
        toolchain: "Toolchain",
        name: str,
        srcs: list["artifact"],
        output: "artifact") -> "artifact":
    """build an app_info.term file that contains the meta information for building the .app file"""
    sources_args = convert(srcs)
    sources_args.ignore_artifacts()
    data = {
        "applications": [
            app[ErlangAppInfo].name
            for app in ctx.attrs.applications
        ],
        "included_applications": [
            app[ErlangAppInfo].name
            for app in ctx.attrs.included_applications
        ],
        "name": name,
        "output": output,
        "sources": sources_args,
    }
    if ctx.attrs.version:
        data["version"] = ctx.attrs.version
    if ctx.attrs.app_src:
        data["template"] = ctx.attrs.app_src
    if ctx.attrs.mod:
        data["mod"] = ctx.attrs.mod
    if ctx.attrs.env:
        data["env"] = {k: cmd_args(v) for k, v in ctx.attrs.env.items()}
    if ctx.attrs.extra_properties:
        data["metadata"] = {k: normalise_metadata(v) for k, v in ctx.attrs.extra_properties.items()}

    app_info_content = to_term_args(data)
    return ctx.actions.write(
        paths.join(erlang_build.utils.build_dir(toolchain), "app_info.term"),
        app_info_content,
    )

def link_output(
        ctx: AnalysisContext,
        link_path: str,
        build_environment: "BuildEnvironment") -> "artifact":
    """Link application output folder in working dir root folder."""
    name = ctx.attrs.name

    ebin = build_environment.app_beams.values() + [build_environment.app_files[name]]
    include = build_environment.app_includes.values()
    chunks = build_environment.app_chunks.values()
    priv = build_environment.priv_dirs[name]

    ebin = {
        paths.join("ebin", ebin_file.basename): ebin_file
        for ebin_file in ebin
    }

    include = {
        paths.join("include", include_file.basename): include_file
        for include_file in include
    }

    srcs = _link_srcs_folder(ctx)

    if getattr(ctx.attrs, "build_edoc_chunks", False):
        edoc = {
            paths.join("doc", "chunks", chunk_file.basename): chunk_file
            for chunk_file in chunks
        }
    else:
        edoc = {}

    link_spec = {}
    link_spec.update(ebin)
    link_spec.update(include)
    link_spec.update(srcs)
    link_spec.update(edoc)
    link_spec["priv"] = priv

    return ctx.actions.symlinked_dir(link_path, link_spec)

def _link_srcs_folder(ctx: AnalysisContext) -> dict[str, "artifact"]:
    """Build mapping for the src folder if erlang.include_src is set"""
    if not str_to_bool(read_root_config("erlang", "include_src", "False")):
        return {}
    srcs = {
        paths.join("src", src_file.basename): src_file
        for src_file in ctx.attrs.srcs
    }
    if ctx.attrs.app_src:
        srcs[paths.join("src", ctx.attrs.app_src.basename)] = ctx.attrs.app_src
    return srcs

def _build_start_dependencies(ctx: AnalysisContext, toolchain: "Toolchain") -> list["StartDependencySet"]:
    return build_apps_start_dependencies(
        ctx,
        toolchain,
        [(app, StartType("permanent")) for app in ctx.attrs.applications],
    ) + build_apps_start_dependencies(
        ctx,
        toolchain,
        [(app, StartType("load")) for app in ctx.attrs.included_applications],
    )

def build_apps_start_dependencies(ctx: AnalysisContext, toolchain: "Toolchain", apps: list[(Dependency, "StartType")]) -> list["StartDependencySet"]:
    start_dependencies = []
    for app, start_type in apps[::-1]:
        app_spec = _build_start_spec(toolchain, app[ErlangAppInfo], start_type)

        if app[ErlangAppInfo].virtual:
            children = []
        else:
            children = app[ErlangAppInfo].start_dependencies[toolchain.name]

        app_set = ctx.actions.tset(
            StartDependencySet,
            value = app_spec,
            children = children,
        )

        start_dependencies.append(app_set)

    return start_dependencies

def _build_start_spec(toolchain: "Toolchain", app_info: "provider", start_type: "StartType") -> "StartSpec":
    if app_info.version == "dynamic":
        version = app_info.version
    else:
        version = app_info.version[toolchain.name]

    return StartSpec(
        name = app_info.name,
        version = version,
        resolved = not app_info.virtual,
        start_type = start_type,
    )

def _build_default_info(dependencies: ErlAppDependencies, app_dir: "artifact") -> "provider":
    """ generate default_outputs and DefaultInfo provider
    """

    outputs = [
        dep[ErlangAppInfo].app_folder
        for dep in dependencies.values()
        if ErlangAppInfo in dep and
           not dep[ErlangAppInfo].virtual
    ]

    return DefaultInfo(default_output = app_dir, other_outputs = outputs)

def build_app_info(
        ctx: AnalysisContext,
        dependencies: ErlAppDependencies,
        build_environments: dict[str, "BuildEnvironment"],
        app_folders: dict[str, "artifact"],
        primary_app_folder: "artifact",
        start_dependencies: dict[str, list["StartDependencySet"]]) -> "provider":
    name = ctx.attrs.name

    version = {
        toolchain.name: ctx.attrs.version
        for toolchain in select_toolchains(ctx).values()
    }

    # build application info
    return ErlangAppInfo(
        name = name,
        version = version,
        beams = multidict_projection(build_environments, "app_beams"),
        includes = multidict_projection(build_environments, "app_includes"),
        dependencies = dependencies,
        start_dependencies = start_dependencies,
        app_file = multidict_projection_key(build_environments, "app_files", name),
        priv_dir = multidict_projection_key(build_environments, "priv_dirs", name),
        include_dir = multidict_projection_key(build_environments, "include_dirs", name),
        ebin_dir = multidict_projection_key(build_environments, "ebin_dirs", name),
        private_include_dir = multidict_projection(build_environments, "private_include_dir"),
        private_includes = multidict_projection(build_environments, "private_includes"),
        deps_files = multidict_projection(build_environments, "deps_files"),
        input_mapping = multidict_projection(build_environments, "input_mapping"),
        virtual = False,
        app_folders = app_folders,
        app_folder = primary_app_folder,
    )
