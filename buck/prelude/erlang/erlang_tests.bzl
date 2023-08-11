# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:paths.bzl", "paths")
load(
    ":erlang_build.bzl",
    "BuildEnvironment",
    "erlang_build",
    "module_name",
)
load(
    ":erlang_dependencies.bzl",
    "ErlAppDependencies",
    "check_dependencies",
    "flatten_dependencies",
)
load(":erlang_info.bzl", "ErlangAppInfo", "ErlangTestInfo")
load(":erlang_otp_application.bzl", "normalize_application")
load(":erlang_shell.bzl", "erlang_shell")
load(
    ":erlang_toolchain.bzl",
    "get_primary",
    "select_toolchains",
)
load(
    ":erlang_utils.bzl",
    "file_mapping",
    "list_dedupe",
    "preserve_structure",
    "to_term_args",
)

def erlang_tests_macro(
        erlang_app_rule,
        erlang_test_rule,
        suites: list[str],
        deps: list[str] = [],
        resources: list[str] = [],
        property_tests: list[str] = [],
        config_files: list[str] = [],
        srcs: list[str] = [],
        use_default_configs: bool = True,
        use_default_deps: bool = True,
        **common_attributes: dict) -> None:
    """
    Generate multiple erlang_test targets based on the `suites` field.
    Also adds the default 'config' and 'deps' from the buck2 config.
    The macro also produces and adds
    resource targets for files in the suite associated <suitename>_data folder.
    """
    deps = [normalize_application(dep) for dep in deps]
    config_files = list(config_files)

    if not suites:
        return

    if srcs:
        # There is no "good name" for the application
        # We create one using the first suite from the list
        (suite_name, _ext) = paths.split_extension(paths.basename(suites[0]))
        srcs_app = suite_name + "_app"
        app_deps = [dep for dep in deps if not dep.endswith("_SUITE")]
        erlang_app_rule(
            name = srcs_app,
            srcs = srcs,
            labels = ["generated", "test_application", "test_utils"],
            applications = app_deps,
        )
        deps.append(":" + srcs_app)

    # add default apps

    default_deps = read_root_config("erlang", "erlang_tests_default_apps", None) if use_default_deps else None
    default_config_files = read_root_config("erlang", "erlang_tests_default_config", None) if use_default_configs else None
    trampoline = read_root_config("erlang", "erlang_tests_trampoline", None) if use_default_configs else None
    providers = read_root_config("erlang", "erlang_test_providers", "") if use_default_configs else ""

    if default_config_files:
        config_files += default_config_files.split()

    if default_deps != None:
        deps += default_deps.split()

    target_resources = list(resources)

    if not property_tests:
        first_suite = suites[0]
        prop_target = generate_file_map_target(first_suite, "property_test")
        if prop_target:
            property_tests = [prop_target]

    common_attributes["labels"] = common_attributes.get("labels", []) + ["tpx-enable-artifact-reporting", "test-framework=39:erlang_common_test"]

    additional_labels = read_config("erlang", "test_labels", None)
    if additional_labels != None:
        common_attributes["labels"] += additional_labels.split()

    common_attributes["labels"] = list_dedupe(common_attributes["labels"])

    for suite in suites:
        # forward resources and deps fields and generate erlang_test target
        (suite_name, _ext) = paths.split_extension(paths.basename(suite))
        if not suite_name.endswith("_SUITE"):
            fail("erlang_tests target only accept suite as input, found " + suite_name)

        # check if there is a data folder and add it as resource if existing
        data_dir_name = "{}_data".format(suite_name)
        suite_resource = target_resources
        data_target = generate_file_map_target(suite, data_dir_name)
        if data_target:
            suite_resource = [target for target in target_resources]
            suite_resource.append(data_target)

        # forward resources and deps fields and generate erlang_test target
        erlang_test_rule(
            name = suite_name,
            suite = suite,
            deps = deps,
            resources = suite_resource,
            config_files = config_files,
            property_tests = property_tests,
            _trampoline = trampoline,
            _providers = providers,
            **common_attributes
        )

def erlang_test_impl(ctx: AnalysisContext) -> list["provider"]:
    toolchains = select_toolchains(ctx)
    primary_toolchain_name = get_primary(ctx)
    primary_toolchain = toolchains[primary_toolchain_name]

    deps = ctx.attrs.deps + [ctx.attrs._test_binary_lib]

    # collect all dependencies
    all_direct_dependencies = check_dependencies(deps, [ErlangAppInfo, ErlangTestInfo])
    dependencies = flatten_dependencies(ctx, all_direct_dependencies)

    # prepare build environment
    pre_build_environment = erlang_build.prepare_build_environment(ctx, primary_toolchain, dependencies)

    new_private_include_dir = pre_build_environment.private_include_dir

    # pre_build_environment.private_includes is immutable, that's how we change that.
    new_private_includes = {a: b for (a, b) in pre_build_environment.private_includes.items()}

    #Pull private deps from dependencies
    for dep in dependencies.values():
        if ErlangAppInfo in dep:
            if dep[ErlangAppInfo].private_include_dir:
                new_private_include_dir = new_private_include_dir + dep[ErlangAppInfo].private_include_dir[primary_toolchain_name]
                new_private_includes.update(dep[ErlangAppInfo].private_includes[primary_toolchain_name])

    # Records are immutable, hence we need to create a new record from the previous one.
    build_environment = BuildEnvironment(
        includes = pre_build_environment.includes,
        private_includes = new_private_includes,
        beams = pre_build_environment.beams,
        priv_dirs = pre_build_environment.priv_dirs,
        include_dirs = pre_build_environment.include_dirs,
        private_include_dir = new_private_include_dir,
        ebin_dirs = pre_build_environment.ebin_dirs,
        deps_files = pre_build_environment.deps_files,
        app_files = pre_build_environment.app_files,
        full_dependencies = pre_build_environment.full_dependencies,
        # convenience storrage
        app_includes = pre_build_environment.app_includes,
        app_beams = pre_build_environment.app_beams,
        app_chunks = pre_build_environment.app_chunks,
        # input mapping
        input_mapping = pre_build_environment.input_mapping,
    )

    # Config files for ct
    config_files = [config_file[DefaultInfo].default_outputs[0] for config_file in ctx.attrs.config_files]

    test_binary = ctx.attrs._test_binary[RunInfo]

    trampoline = ctx.attrs._trampoline
    cmd = cmd_args([])
    if trampoline:
        cmd.add(trampoline[RunInfo])

    cmd.add(test_binary)

    suite = ctx.attrs.suite
    suite_name = module_name(suite)

    build_environment = erlang_build.build_steps.generate_beam_artifacts(
        ctx,
        primary_toolchain,
        build_environment,
        "tests",
        [suite],
    )

    ebin_dir = paths.dirname(build_environment.ebin_dirs["tests"].short_path)

    suite_data = paths.join(ebin_dir, suite_name + "_data")
    data_dir = _build_resource_dir(ctx, ctx.attrs.resources, suite_data)
    property_dir = _build_resource_dir(ctx, ctx.attrs.property_tests, paths.join(ebin_dir, "property_test"))

    output_dir = link_output(ctx, suite_name, build_environment, data_dir, property_dir)
    test_info_file = _write_test_info_file(
        ctx = ctx,
        test_suite = suite_name,
        dependencies = dependencies,
        test_dir = output_dir,
        config_files = config_files,
        erl_cmd = primary_toolchain.otp_binaries.erl,
    )
    cmd.add(test_info_file)

    default_info = _build_default_info(dependencies, output_dir)
    for output_artifact in default_info.other_outputs:
        cmd.hidden(output_artifact)
    for config_file in config_files:
        cmd.hidden(config_file)

    cmd.hidden(output_dir)

    # prepare shell dependencies
    additional_paths = [
        dep[ErlangTestInfo].output_dir
        for dep in dependencies.values()
        if ErlangTestInfo in dep
    ] + [output_dir]

    preamble = '-eval "%s" \\' % (ctx.attrs.preamble)
    additional_args = [cmd_args(preamble, "-noshell \\")]

    all_direct_shell_dependencies = check_dependencies([ctx.attrs._cli_lib], [ErlangAppInfo])
    cli_lib_deps = flatten_dependencies(ctx, all_direct_shell_dependencies)

    shell_deps = dict(dependencies)
    shell_deps.update(**{name: dep for (name, dep) in cli_lib_deps.items() if ErlangAppInfo in dep})

    run_info = erlang_shell.build_run_info(
        ctx,
        shell_deps.values(),
        additional_paths = additional_paths,
        additional_args = additional_args,
    )

    return [
        default_info,
        run_info,
        ExternalRunnerTestInfo(
            type = "erlang_test",
            command = [cmd],
            env = ctx.attrs.env,
            labels = ["tpx-fb-test-type=16"] + ctx.attrs.labels,
            contacts = ctx.attrs.contacts,
            run_from_project_root = True,
        ),
        ErlangTestInfo(
            name = suite_name,
            dependencies = dependencies,
            output_dir = output_dir,
        ),
    ]

# Copied from erlang_application.
def _build_default_info(dependencies: ErlAppDependencies, output_dir: "artifact") -> "provider":
    """ generate default_outputs and DefaultInfo provider
    """
    outputs = []
    for dep in dependencies.values():
        if ErlangAppInfo in dep and not dep[ErlangAppInfo].virtual:
            outputs.append(dep[ErlangAppInfo].app_folder)
        if ErlangTestInfo in dep:
            outputs += dep[DefaultInfo].default_outputs
            outputs += dep[DefaultInfo].other_outputs
    return DefaultInfo(default_output = output_dir, other_outputs = outputs)

def _write_test_info_file(
        ctx: AnalysisContext,
        test_suite: str,
        dependencies: ErlAppDependencies,
        test_dir: "artifact",
        config_files: list["artifact"],
        erl_cmd: [cmd_args, "artifact"]) -> "artifact":
    tests_info = {
        "config_files": config_files,
        "ct_opts": ctx.attrs._ct_opts,
        "dependencies": _list_code_paths(dependencies),
        "erl_cmd": cmd_args(['"', cmd_args(erl_cmd, delimiter = " "), '"'], delimiter = ""),
        "extra_ct_hooks": ctx.attrs.extra_ct_hooks,
        "providers": ctx.attrs._providers,
        "test_dir": test_dir,
        "test_suite": test_suite,
    }
    test_info_file = ctx.actions.declare_output("tests_info")
    ctx.actions.write(
        test_info_file,
        to_term_args(tests_info),
    )
    return test_info_file

def _list_code_paths(dependencies: ErlAppDependencies) -> list[cmd_args]:
    """lists all ebin/ dirs from the test targets dependencies"""
    folders = []
    for dependency in dependencies.values():
        if ErlangAppInfo in dependency:
            dep_info = dependency[ErlangAppInfo]
            if dep_info.virtual:
                continue
            folders.append(cmd_args(
                dep_info.app_folder,
                format = '"{}/ebin"',
            ))
        elif ErlangTestInfo in dependency:
            dep_info = dependency[ErlangTestInfo]
            folders.append(cmd_args(dep_info.output_dir, format = '"{}"'))
    return folders

def _build_resource_dir(ctx, resources: list, target_dir: str) -> "artifact":
    """ build mapping for suite data directory

    generating the necessary mapping information for the suite data directory
    the resulting mapping can be used directly to symlink
    """
    include_symlinks = {}
    for resource in resources:
        files = resource[DefaultInfo].default_outputs
        for file in files:
            include_symlinks[file.short_path] = file
    return ctx.actions.symlinked_dir(
        target_dir,
        include_symlinks,
    )

def link_output(
        ctx: AnalysisContext,
        test_suite: str,
        build_environment: "BuildEnvironment",
        data_dir: "artifact",
        property_dir: "artifact") -> "artifact":
    """Link the data_dirs and the test_suite beam in a single output folder."""
    link_spec = {}
    beam = build_environment.app_beams[test_suite]
    link_spec[beam.basename] = beam
    link_spec[data_dir.basename] = data_dir
    link_spec[property_dir.basename] = property_dir
    link_spec[ctx.attrs.suite.basename] = ctx.attrs.suite
    return ctx.actions.symlinked_dir(ctx.attrs.name, link_spec)

def generate_file_map_target(suite: str, dir_name: str) -> str:
    suite_dir = paths.dirname(suite)
    suite_name = paths.basename(suite)
    files = glob([paths.join(suite_dir, dir_name, "**")])
    if len(files):
        # generate target for data dir
        file_mapping(
            name = "{}-{}".format(dir_name, suite_name),
            mapping = preserve_structure(
                path = paths.join(suite_dir, dir_name),
            ),
        )
        return ":{}-{}".format(dir_name, suite_name)
    return ""
