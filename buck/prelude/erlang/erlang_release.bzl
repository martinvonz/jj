# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:paths.bzl", "paths")
load(
    ":erlang_application.bzl",
    "StartDependencySet",
    "StartSpec",
    "StartType",
    "build_apps_start_dependencies",
)
load(":erlang_build.bzl", "erlang_build")
load(":erlang_dependencies.bzl", "ErlAppDependencies", "check_dependencies", "flatten_dependencies")
load(
    ":erlang_info.bzl",
    "ErlangAppInfo",
    "ErlangReleaseInfo",
    "ErlangToolchainInfo",
)
load(":erlang_toolchain.bzl", "get_primary", "select_toolchains")
load(":erlang_utils.bzl", "action_identifier", "to_term_args")

# Erlang Releases according to https://www.erlang.org/doc/design_principles/release_structure.html

def erlang_release_impl(ctx: AnalysisContext) -> list["provider"]:
    root_apps = check_dependencies(_dependencies(ctx), [ErlangAppInfo])
    all_apps = flatten_dependencies(ctx, root_apps)

    if ctx.attrs.multi_toolchain != None:
        return _build_multi_toolchain_releases(ctx, all_apps, ctx.attrs.multi_toolchain)
    else:
        return _build_primary_release(ctx, all_apps)

def _build_multi_toolchain_releases(
        ctx: AnalysisContext,
        apps: ErlAppDependencies,
        configured_toolchains: list[Dependency]) -> list["provider"]:
    """build the release for all toolchains with the structure being releases/<toolchain>/<relname>"""
    all_toolchains = select_toolchains(ctx)
    toolchains = _get_configured_toolchains(all_toolchains, configured_toolchains)
    outputs = {}
    for toolchain in toolchains.values():
        outputs[toolchain.name] = _build_release(ctx, toolchain, apps)
    releases_dir = _symlink_multi_toolchain_output(ctx, outputs)
    return [DefaultInfo(default_output = releases_dir), ErlangReleaseInfo(name = _relname(ctx))]

def _get_configured_toolchains(
        toolchains: dict[str, "Toolchain"],
        configured_toolchains: list[Dependency]) -> dict[str, "Toolchain"]:
    retval = {}
    for dep in configured_toolchains:
        if not dep[ErlangToolchainInfo]:
            fail("{} is not a valid toolchain target".format(dep))

        toolchain_info = dep[ErlangToolchainInfo]
        retval[toolchain_info.name] = toolchains[toolchain_info.name]
    return retval

def _build_primary_release(ctx: AnalysisContext, apps: ErlAppDependencies) -> list["provider"]:
    """build the release only with the primary toolchain with the release folder on the top-level"""
    toolchains = select_toolchains(ctx)
    primary_toolchain = toolchains[get_primary(ctx)]
    all_outputs = _build_release(ctx, primary_toolchain, apps)
    release_dir = _symlink_primary_toolchain_output(ctx, all_outputs)
    return [DefaultInfo(default_output = release_dir), ErlangReleaseInfo(name = _relname(ctx))]

def _build_release(ctx: AnalysisContext, toolchain: "Toolchain", apps: ErlAppDependencies) -> dict[str, "artifact"]:
    # OTP base structure
    lib_dir = _build_lib_dir(ctx, toolchain, apps)
    boot_scripts = _build_boot_script(ctx, toolchain, lib_dir["lib"])

    # release specific variables in bin/release_variables
    release_variables = _build_release_variables(ctx, toolchain)

    # Overlays
    overlays = _build_overlays(ctx, toolchain)

    # erts
    maybe_erts = _build_erts(ctx, toolchain)

    # link output
    all_outputs = {}
    for outputs in [
        lib_dir,
        boot_scripts,
        overlays,
        release_variables,
        maybe_erts,
    ]:
        all_outputs.update(outputs)

    return all_outputs

def _build_lib_dir(ctx: AnalysisContext, toolchain: "Toolchain", all_apps: ErlAppDependencies) -> dict[str, "artifact"]:
    """Build lib dir according to OTP specifications.

    .. seealso:: `OTP Design Principles Release Structure <https://www.erlang.org/doc/design_principles/release_structure.html>`_
    """
    release_name = _relname(ctx)
    build_dir = erlang_build.utils.build_dir(toolchain)

    link_spec = {
        _app_folder(toolchain, dep): dep[ErlangAppInfo].app_folders[toolchain.name]
        for dep in all_apps.values()
        if ErlangAppInfo in dep and not dep[ErlangAppInfo].virtual
    }

    lib_dir = ctx.actions.symlinked_dir(
        paths.join(build_dir, release_name, "lib"),
        link_spec,
    )
    return {"lib": lib_dir}

def _build_boot_script(
        ctx: AnalysisContext,
        toolchain: "Toolchain",
        lib_dir: "artifact") -> dict[str, "artifact"]:
    """Build Name.rel, start.script, and start.boot in the release folder."""
    release_name = _relname(ctx)
    build_dir = erlang_build.utils.build_dir(toolchain)

    start_type_mapping = _dependencies_with_start_types(ctx)
    root_apps = check_dependencies(_dependencies(ctx), [ErlangAppInfo])
    root_apps_names = [app[ErlangAppInfo].name for app in root_apps]

    root_apps_with_start_type = [
        (app, start_type_mapping[_app_name(app)])
        for app in root_apps
    ]
    start_dependencies = build_apps_start_dependencies(ctx, toolchain, root_apps_with_start_type)

    root_set = ctx.actions.tset(
        StartDependencySet,
        value = StartSpec(
            name = "__ignored__",
            version = ctx.attrs.version,
            start_type = StartType("permanent"),
            resolved = False,
        ),
        children = start_dependencies,
    )

    reverse_start_order = list(root_set.traverse())
    reverse_start_order.pop(0)

    seen = {}
    release_applications = []
    root_apps_spec = {}
    for spec in reverse_start_order[::-1]:
        if spec.name in seen:
            continue
        seen[spec.name] = True

        app_spec = {
            "name": spec.name,
            "resolved": str(spec.resolved),
            "type": spec.start_type.value,
            "version": spec.version,
        }

        if spec.name in root_apps_names:
            root_apps_spec[spec.name] = app_spec
        else:
            release_applications.append(app_spec)
    release_applications = [root_apps_spec[app_name] for app_name in root_apps_names] + release_applications

    data = {
        "apps": release_applications,
        "lib_dir": lib_dir,
        "name": release_name,
        "version": ctx.attrs.version,
    }

    content = to_term_args(data)
    spec_file = ctx.actions.write(paths.join(build_dir, "boot_script_spec.term"), content)

    releases_dir = paths.join(build_dir, release_name, "releases", ctx.attrs.version)

    release_resource = ctx.actions.declare_output(paths.join(releases_dir, "%s.rel" % (release_name,)))
    start_script = ctx.actions.declare_output(paths.join(releases_dir, "start.script"))
    boot_script = ctx.actions.declare_output(paths.join(releases_dir, "start.boot"))

    script = toolchain.boot_script_builder
    boot_script_build_cmd = cmd_args(
        [
            toolchain.otp_binaries.escript,
            script,
            spec_file,
            cmd_args(release_resource.as_output()).parent(),
        ],
    )
    boot_script_build_cmd.hidden(start_script.as_output())
    boot_script_build_cmd.hidden(boot_script.as_output())
    boot_script_build_cmd.hidden(lib_dir)

    erlang_build.utils.run_with_env(
        ctx,
        toolchain,
        boot_script_build_cmd,
        category = "build_boot_script",
        identifier = action_identifier(toolchain, release_name),
    )

    return {
        paths.join("releases", ctx.attrs.version, file.basename): file
        for file in [
            release_resource,
            start_script,
            boot_script,
        ]
    }

def _build_overlays(ctx: AnalysisContext, toolchain: "Toolchain") -> dict[str, "artifact"]:
    release_name = _relname(ctx)
    build_dir = erlang_build.utils.build_dir(toolchain)
    installed = {}
    for target, deps in ctx.attrs.overlays.items():
        for dep in deps:
            for artifact in dep[DefaultInfo].default_outputs + dep[DefaultInfo].other_outputs:
                build_path = paths.normalize(paths.join(build_dir, release_name, target, artifact.basename))
                link_path = paths.normalize(paths.join(target, artifact.basename))
                if link_path in installed:
                    fail("multiple overlays defined for the same location: %s" % (link_path,))
                installed[link_path] = ctx.actions.copy_file(build_path, artifact)
    return installed

def _build_release_variables(ctx: AnalysisContext, toolchain: "Toolchain") -> dict[str, "artifact"]:
    release_name = _relname(ctx)

    releases_dir = paths.join(
        erlang_build.utils.build_dir(toolchain),
        release_name,
        "releases",
        ctx.attrs.version,
    )
    short_path = paths.join("bin", "release_variables")
    release_variables = ctx.actions.declare_output(
        paths.join(releases_dir, short_path),
    )

    spec_file = ctx.actions.write(
        paths.join(erlang_build.utils.build_dir(toolchain), "relvars.term"),
        to_term_args({"variables": {
            "REL_NAME": release_name,
            "REL_VSN": ctx.attrs.version,
        }}),
    )

    release_variables_build_cmd = cmd_args([
        toolchain.otp_binaries.escript,
        toolchain.release_variables_builder,
        spec_file,
        release_variables.as_output(),
    ])

    erlang_build.utils.run_with_env(
        ctx,
        toolchain,
        release_variables_build_cmd,
        category = "build_release_variables",
        identifier = action_identifier(toolchain, release_name),
    )
    return {short_path: release_variables}

def _build_erts(ctx: AnalysisContext, toolchain: "Toolchain") -> dict[str, "artifact"]:
    if not ctx.attrs.include_erts:
        return {}

    release_name = _relname(ctx)

    short_path = "erts"

    erts_dir = paths.join(
        erlang_build.utils.build_dir(toolchain),
        release_name,
        short_path,
    )

    output_artifact = ctx.actions.declare_output(erts_dir)
    ctx.actions.run(
        cmd_args([
            toolchain.otp_binaries.escript,
            toolchain.include_erts,
            output_artifact.as_output(),
        ]),
        category = "include_erts",
        identifier = action_identifier(toolchain, release_name),
    )

    return {short_path: output_artifact}

def _symlink_multi_toolchain_output(ctx: AnalysisContext, toolchain_artifacts: dict[str, dict[str, "artifact"]]) -> "artifact":
    link_spec = {}
    relname = _relname(ctx)

    for toolchain_name, artifacts in toolchain_artifacts.items():
        prefix = paths.join(toolchain_name, relname)
        link_spec.update({
            paths.join(prefix, path): artifact
            for path, artifact in artifacts.items()
        })

    return ctx.actions.symlinked_dir(
        "releases",
        link_spec,
    )

def _symlink_primary_toolchain_output(ctx: AnalysisContext, artifacts: dict[str, "artifact"]) -> "artifact":
    return ctx.actions.symlinked_dir(
        _relname(ctx),
        artifacts,
    )

def _relname(ctx: AnalysisContext) -> str:
    return ctx.attrs.release_name if ctx.attrs.release_name else ctx.attrs.name

def _app_folder(toolchain: "Toolchain", dep: Dependency) -> str:
    """Build folder names (i.e., name-version) from an Erlang Application dependency."""
    return "%s-%s" % (dep[ErlangAppInfo].name, dep[ErlangAppInfo].version[toolchain.name])

def _dependencies(ctx: AnalysisContext) -> list[Dependency]:
    """Extract dependencies from `applications` field, order preserving"""
    deps = []
    for dep in ctx.attrs.applications:
        if type(dep) == "tuple":
            deps.append(dep[0])
        else:
            deps.append(dep)
    return deps

def _dependencies_with_start_types(ctx: AnalysisContext) -> dict[str, "StartType"]:
    """Extract mapping from dependency to start type from `applications` field, this is not order preserving"""
    deps = {}
    for dep in ctx.attrs.applications:
        if type(dep) == "tuple":
            deps[_app_name(dep[0])] = StartType(dep[1])
        else:
            deps[_app_name(dep)] = StartType("permanent")
    return deps

def _app_name(app: Dependency) -> str:
    """Helper to unwrap the name for an erlang application dependency"""
    return app[ErlangAppInfo].name
