# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(
    "@prelude//:paths.bzl",
    "paths",
)
load(
    ":erlang_info.bzl",
    "ErlangMultiVersionToolchainInfo",
    "ErlangOTPBinariesInfo",
    "ErlangParseTransformInfo",
    "ErlangToolchainInfo",
)

Tool = "cmd_args"

ToolsBinaries = record(
    erl = field("artifact"),
    erlc = field("artifact"),
    escript = field("artifact"),
)

Tools = record(
    name = field("string"),
    erl = field(Tool),
    erlc = field(Tool),
    escript = field(Tool),
    _tools_binaries = field("ToolsBinaries"),
)

Toolchain = record(
    name = field("string"),
    erl_opts = field(["string"]),
    app_file_script = field("artifact"),
    boot_script_builder = field("artifact"),
    dependency_analyzer = field("artifact"),
    erlc_trampoline = field("artifact"),
    escript_builder = field("artifact"),
    otp_binaries = field("Tools"),
    release_variables_builder = field("artifact"),
    include_erts = field("artifact"),
    core_parse_transforms = field({"string": ("artifact", "artifact")}),
    parse_transforms = field({"string": ("artifact", "artifact")}),
    parse_transforms_filters = field({"string": ["string"]}),
    edoc = field("artifact"),
    edoc_options = field(["string"]),
    utility_modules = field("artifact"),
    env = field({"string": "string"}),
)

ToolchainUtillInfo = provider(
    fields = [
        "app_src_script",
        "boot_script_builder",
        "core_parse_transforms",
        "dependency_analyzer",
        "edoc",
        "erlc_trampoline",
        "escript_builder",
        "release_variables_builder",
        "include_erts",
        "utility_modules",
    ],
)

def select_toolchains(ctx: AnalysisContext) -> dict[str, "Toolchain"]:
    """helper returning toolchains"""
    return ctx.attrs._toolchain[ErlangMultiVersionToolchainInfo].toolchains

def get_primary(ctx: AnalysisContext) -> str:
    return ctx.attrs._toolchain[ErlangMultiVersionToolchainInfo].primary

def get_primary_tools(ctx: AnalysisContext) -> "Tools":
    return (get_primary_toolchain(ctx)).otp_binaries

def get_primary_toolchain(ctx: AnalysisContext) -> "Toolchain":
    return (select_toolchains(ctx)[get_primary(ctx)])

def _multi_version_toolchain_impl(ctx: AnalysisContext) -> list["provider"]:
    toolchains = {}
    for toolchain in ctx.attrs.targets:
        toolchain_info = toolchain[ErlangToolchainInfo]
        toolchains[toolchain_info.name] = Toolchain(
            name = toolchain_info.name,
            app_file_script = toolchain_info.app_file_script,
            boot_script_builder = toolchain_info.boot_script_builder,
            dependency_analyzer = toolchain_info.dependency_analyzer,
            erl_opts = toolchain_info.erl_opts,
            erlc_trampoline = toolchain_info.erlc_trampoline,
            escript_builder = toolchain_info.escript_builder,
            otp_binaries = toolchain_info.otp_binaries,
            release_variables_builder = toolchain_info.release_variables_builder,
            include_erts = toolchain_info.include_erts,
            core_parse_transforms = toolchain_info.core_parse_transforms,
            parse_transforms = toolchain_info.parse_transforms,
            parse_transforms_filters = toolchain_info.parse_transforms_filters,
            edoc = toolchain_info.edoc,
            edoc_options = toolchain_info.edoc_options,
            utility_modules = toolchain_info.utility_modules,
            env = toolchain_info.env,
        )
    return [
        DefaultInfo(),
        ErlangMultiVersionToolchainInfo(
            toolchains = toolchains,
            primary = ctx.attrs.targets[0][ErlangToolchainInfo].name,
        ),
    ]

multi_version_toolchain_rule = rule(
    impl = _multi_version_toolchain_impl,
    attrs = {
        "targets": attrs.list(attrs.dep()),
    },
    is_toolchain_rule = True,
)

def as_target(name: str) -> str:
    return ":" + name

def _config_erlang_toolchain_impl(ctx: AnalysisContext) -> list["provider"]:
    """ rule for erlang toolchain
    """

    # split the options string to get a list of options
    erl_opts = ctx.attrs.erl_opts.split()
    emu_flags = ctx.attrs.emu_flags.split()
    edoc_options = ctx.attrs.edoc_options.split()

    # get otp binaries
    binaries_info = ctx.attrs.otp_binaries[ErlangOTPBinariesInfo]
    erl = cmd_args([binaries_info.erl] + emu_flags)
    erlc = cmd_args(binaries_info.erlc)
    escript = cmd_args(binaries_info.escript)
    erlc.hidden(binaries_info.erl)
    escript.hidden(binaries_info.erl)
    tools_binaries = ToolsBinaries(
        erl = binaries_info.erl,
        erlc = binaries_info.erl,
        escript = binaries_info.escript,
    )
    otp_binaries = Tools(
        name = ctx.attrs.name,
        erl = erl,
        erlc = erlc,
        escript = escript,
        _tools_binaries = tools_binaries,
    )

    # extract utility artefacts
    utils = ctx.attrs.toolchain_utilities[ToolchainUtillInfo]

    core_parse_transforms = _gen_parse_transforms(
        ctx,
        otp_binaries.erlc,
        utils.core_parse_transforms,
    )

    parse_transforms = _gen_parse_transforms(
        ctx,
        otp_binaries.erlc,
        ctx.attrs.parse_transforms,
    )
    intersection = [key for key in parse_transforms if key in core_parse_transforms]
    if len(intersection):
        fail("conflicting parse_transform with core parse_transform found: %s" % (repr(intersection),))

    utility_modules = _gen_util_beams(ctx, utils.utility_modules, otp_binaries.erlc)

    return [
        DefaultInfo(),
        ErlangToolchainInfo(
            name = ctx.attrs.name,
            app_file_script = utils.app_src_script,
            boot_script_builder = utils.boot_script_builder,
            dependency_analyzer = utils.dependency_analyzer,
            erl_opts = erl_opts,
            env = ctx.attrs.env,
            emu_flags = emu_flags,
            erlc_trampoline = utils.erlc_trampoline,
            escript_builder = utils.escript_builder,
            otp_binaries = otp_binaries,
            release_variables_builder = utils.release_variables_builder,
            include_erts = utils.include_erts,
            core_parse_transforms = core_parse_transforms,
            parse_transforms = parse_transforms,
            parse_transforms_filters = ctx.attrs.parse_transforms_filters,
            edoc = utils.edoc,
            edoc_options = edoc_options,
            utility_modules = utility_modules,
        ),
    ]

def _configured_otp_binaries_impl(ctx: AnalysisContext) -> list["provider"]:
    name = ctx.attrs.name
    tools = get_primary_tools(ctx)
    bin_dir = ctx.actions.symlinked_dir(
        name,
        {
            "erl": tools._tools_binaries.erl,
            "erlc": tools._tools_binaries.erlc,
            "escript": tools._tools_binaries.escript,
        },
    )
    return [
        DefaultInfo(
            default_output = bin_dir,
            sub_targets = {
                "erl": [DefaultInfo(default_output = tools._tools_binaries.erl), RunInfo(tools.erl)],
                "erlc": [DefaultInfo(default_output = tools._tools_binaries.erlc), RunInfo(tools.erlc)],
                "escript": [DefaultInfo(default_output = tools._tools_binaries.escript), RunInfo(tools.escript)],
            },
        ),
    ]

configured_otp_binaries = rule(
    impl = _configured_otp_binaries_impl,
    attrs = {
        "_toolchain": attrs.dep(),
    },
)

def _gen_parse_transforms(ctx: AnalysisContext, erlc: Tool, parse_transforms: list[Dependency]) -> dict[str, ("artifact", "artifact")]:
    transforms = {}
    for dep in parse_transforms:
        src = dep[ErlangParseTransformInfo].source
        extra = dep[ErlangParseTransformInfo].extra_files
        module_name, _ = paths.split_extension(src.basename)
        if module_name in transforms:
            fail("ambiguous global parse_transforms defined: %s", (module_name,))
        transforms[module_name] = _gen_parse_transform_beam(ctx, src, extra, erlc)
    return transforms

def _gen_parse_transform_beam(
        ctx: AnalysisContext,
        src: "artifact",
        extra: list["artifact"],
        erlc: Tool) -> ("artifact", "artifact"):
    name, _ext = paths.split_extension(src.basename)

    # install resources
    resource_dir = ctx.actions.symlinked_dir(
        paths.join(name, "resources"),
        {infile.basename: infile for infile in extra},
    )

    # build beam
    beam = paths.join(
        name,
        paths.replace_extension(src.basename, ".beam"),
    )
    output = ctx.actions.declare_output(beam)

    # NOTE: since we do NOT define +debug_info, this is hermetic
    cmd = cmd_args([
        erlc,
        "+deterministic",
        "-o",
        cmd_args(output.as_output()).parent(),
        src,
    ])
    ctx.actions.run(cmd, category = "erlc", identifier = src.short_path)
    return output, resource_dir

config_erlang_toolchain_rule = rule(
    impl = _config_erlang_toolchain_impl,
    attrs = {
        "core_parse_transforms": attrs.list(attrs.dep(), default = ["@prelude//erlang/toolchain:transform_project_root"]),
        "edoc_options": attrs.string(default = ""),
        "emu_flags": attrs.string(default = ""),
        "env": attrs.dict(key = attrs.string(), value = attrs.string(), default = {}),
        "erl_opts": attrs.string(default = ""),
        "otp_binaries": attrs.dep(),
        "parse_transforms": attrs.list(attrs.dep()),
        "parse_transforms_filters": attrs.dict(key = attrs.string(), value = attrs.list(attrs.string())),
        "toolchain_utilities": attrs.dep(default = "@prelude//erlang/toolchain:toolchain_utilities"),
    },
)

def _gen_util_beams(
        ctx: AnalysisContext,
        sources: list["artifact"],
        erlc: Tool) -> "artifact":
    beams = []
    for src in sources:
        output = ctx.actions.declare_output(paths.join(
            "__build",
            paths.replace_extension(src.basename, ".beam"),
        ))
        ctx.actions.run(
            [
                erlc,
                "+deterministic",
                "-o",
                cmd_args(output.as_output()).parent(),
                src,
            ],
            category = "erlc",
            identifier = src.short_path,
        )
        beams.append(output)

    beam_dir = ctx.actions.symlinked_dir(
        "utility_modules",
        {beam.basename: beam for beam in beams},
    )

    return beam_dir

# Parse Transform

def erlang_otp_binaries_impl(ctx: AnalysisContext):
    erl = ctx.attrs.erl
    erlc = ctx.attrs.erlc
    escript = ctx.attrs.escript
    return [
        DefaultInfo(),
        ErlangOTPBinariesInfo(
            erl = erl,
            erlc = erlc,
            escript = escript,
        ),
    ]

erlang_parse_transform = rule(
    impl = lambda ctx: [
        DefaultInfo(),
        ErlangParseTransformInfo(
            source = ctx.attrs.src,
            extra_files = ctx.attrs.extra_files,
        ),
    ],
    attrs = {
        "extra_files": attrs.list(attrs.source(), default = []),
        "src": attrs.source(),
    },
)

def _toolchain_utils(ctx: AnalysisContext) -> list["provider"]:
    return [
        DefaultInfo(),
        ToolchainUtillInfo(
            app_src_script = ctx.attrs.app_src_script,
            boot_script_builder = ctx.attrs.boot_script_builder,
            core_parse_transforms = ctx.attrs.core_parse_transforms,
            dependency_analyzer = ctx.attrs.dependency_analyzer,
            edoc = ctx.attrs.edoc,
            erlc_trampoline = ctx.attrs.erlc_trampoline,
            escript_builder = ctx.attrs.escript_builder,
            release_variables_builder = ctx.attrs.release_variables_builder,
            include_erts = ctx.attrs.include_erts,
            utility_modules = ctx.attrs.utility_modules,
        ),
    ]

toolchain_utilities = rule(
    impl = _toolchain_utils,
    attrs = {
        "app_src_script": attrs.source(),
        "boot_script_builder": attrs.source(),
        "core_parse_transforms": attrs.list(attrs.dep()),
        "dependency_analyzer": attrs.source(),
        "edoc": attrs.source(),
        "erlc_trampoline": attrs.source(),
        "escript_builder": attrs.source(),
        "include_erts": attrs.source(),
        "release_variables_builder": attrs.source(),
        "utility_modules": attrs.list(attrs.source()),
    },
)

# Resources that need to be plugged in through toolchain// :
# - jsone

toolchain_resources = rule(
    impl = lambda ctx: [
        DefaultInfo(
            sub_targets = {
                "jsone": ctx.attrs.jsone.providers,
            },
        ),
    ],
    attrs = {
        "jsone": attrs.dep(),
    },
    is_toolchain_rule = True,
)

toolchain_resources_internal = rule(
    impl = lambda ctx: ctx.attrs._resources.providers,
    attrs = {
        "_resources": attrs.toolchain_dep(default = "toolchains//:erlang-resources"),
    },
)
