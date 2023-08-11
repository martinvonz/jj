# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:paths.bzl", "paths")
load(":erlang_dependencies.bzl", "ErlAppDependencies")
load(
    ":erlang_info.bzl",
    "ErlangAppIncludeInfo",
    "ErlangAppInfo",
    "ErlangTestInfo",
)
load(":erlang_utils.bzl", "action_identifier", "to_term_args")

# mapping
#   from include base name and application (e.g. ("app1", "header.hrl")
#   to symlinked include/ dir artifact
IncludesMapping = {("string", "string"): "artifact"}

# mapping
#   from include base name (e.g. "header.hrl"
#   to artifact
PathArtifactMapping = {"string": "artifact"}

# mapping
#   from module name
#   to artifact
ModuleArtifactMapping = {"string": "artifact"}

# mapping
#   from input base name
#   path to input artifact from repo root
InputArtifactMapping = {"string": "artifact"}

BuildEnvironment = record(
    includes = field(IncludesMapping, {}),
    private_includes = field(PathArtifactMapping, {}),
    beams = field(ModuleArtifactMapping, {}),
    priv_dirs = field(PathArtifactMapping, {}),
    include_dirs = field(PathArtifactMapping, {}),
    private_include_dir = field(["artifact"], []),
    ebin_dirs = field(PathArtifactMapping, {}),
    deps_files = field(PathArtifactMapping, {}),
    app_files = field(PathArtifactMapping, {}),
    full_dependencies = field(["artifact"], []),
    # convenience storrage
    app_includes = field(IncludesMapping, {}),
    app_beams = field(ModuleArtifactMapping, {}),
    app_chunks = field(ModuleArtifactMapping, {}),
    # input artifact mapping
    input_mapping = field(InputArtifactMapping, {}),
)

def _prepare_build_environment(
        ctx: AnalysisContext,
        toolchain: "Toolchain",
        dependencies: ErlAppDependencies) -> "BuildEnvironment":
    """Prepare build environment and collect the context from all dependencies."""
    priv_dirs = {}
    include_dirs = {}
    ebin_dirs = {}
    deps_files = {}
    includes = {}
    beams = {}
    app_files = {}
    full_dependencies = []
    input_mapping = {}

    # for duplication detection
    apps = {}

    for name, dep in dependencies.items():
        if name in apps:
            fail("duplicated application name found %s" % (name,))
        apps[name] = True

        if ErlangAppInfo in dep:
            dep_info = dep[ErlangAppInfo]

            if dep_info.virtual:
                # virtual applications don't directories we need to include
                # we skip this entire step
                continue

            # collect beams
            intersection = [key for key in beams if key in dep_info.beams[toolchain.name]]
            if len(intersection):
                fail("duplicated modules found in build: {}".format(repr(intersection)))

            beams.update(dep_info.beams[toolchain.name])

            # collect dirs
            priv_dirs[name] = dep_info.priv_dir[toolchain.name]
            ebin_dirs[name] = dep_info.ebin_dir[toolchain.name]

            # collect app files
            app_files[name] = dep_info.app_file[toolchain.name]

        elif ErlangAppIncludeInfo in dep:
            dep_info = dep[ErlangAppIncludeInfo]

            if dep_info.name == ctx.attrs.name:
                continue
        elif ErlangTestInfo in dep:
            # we only care about application deps
            continue
        else:
            fail("invalid dep {}", dep)

        # add transitive input mapping
        # Note: the build will fail if there is ambiguity in the basename
        input_mapping.update(dep_info.input_mapping[toolchain.name])

        # collect includes
        include_dirs[name] = dep_info.include_dir[toolchain.name]
        includes.update(dep_info.includes[toolchain.name])

        # collect deps_files
        deps_files.update(dep_info.deps_files[toolchain.name])

    return BuildEnvironment(
        includes = includes,
        beams = beams,
        priv_dirs = priv_dirs,
        include_dirs = include_dirs,
        ebin_dirs = ebin_dirs,
        deps_files = deps_files,
        app_files = app_files,
        full_dependencies = full_dependencies,
        input_mapping = input_mapping,
    )

def _generate_input_mapping(build_environment: "BuildEnvironment", input_artifacts: list["artifact"]) -> "BuildEnvironment":
    # collect input artifacts for current targets
    # Note: this must be after the dependencies to overwrite private includes
    input_mapping = dict(build_environment.input_mapping)

    for input_artifact in input_artifacts:
        key = input_artifact.basename
        if key in input_mapping and input_mapping[key] != input_artifact:
            fail("conflicting inputs for {}: {} {}".format(key, input_mapping[key], input_artifact))
        input_mapping[key] = input_artifact

    return BuildEnvironment(
        # updated field
        input_mapping = input_mapping,
        # copied fields
        includes = build_environment.includes,
        private_includes = build_environment.private_includes,
        beams = build_environment.beams,
        priv_dirs = build_environment.priv_dirs,
        include_dirs = build_environment.include_dirs,
        private_include_dir = build_environment.private_include_dir,
        ebin_dirs = build_environment.ebin_dirs,
        deps_files = build_environment.deps_files,
        app_files = build_environment.app_files,
        full_dependencies = build_environment.full_dependencies,
        app_includes = build_environment.app_includes,
        app_beams = build_environment.app_beams,
        app_chunks = build_environment.app_chunks,
    )

def _generated_source_artifacts(ctx: AnalysisContext, toolchain: "Toolchain", name: str) -> PathArtifactMapping:
    """Generate source output artifacts and build actions for generated erl files."""
    inputs = [src for src in ctx.attrs.srcs if _is_xyrl(src)]
    outputs = {
        module_name(src): _build_xyrl(
            ctx,
            toolchain,
            src,
            ctx.actions.declare_output(generated_erl_path(toolchain, name, src)),
        )
        for src in inputs
    }
    return outputs

def _generate_include_artifacts(
        ctx: AnalysisContext,
        toolchain: "Toolchain",
        build_environment: "BuildEnvironment",
        name: str,
        header_artifacts: list["artifact"],
        is_private: bool = False) -> "BuildEnvironment":
    # anchor for include dir
    anchor = _make_dir_anchor(ctx, paths.join(_build_dir(toolchain), name, "include"))

    # output artifacts
    include_mapping = {
        _header_key(hrl, name, is_private): ctx.actions.declare_output(anchor_path(anchor, hrl.basename))
        for hrl in header_artifacts
    }

    # dep files
    include_deps = _get_deps_files(ctx, toolchain, anchor, header_artifacts)

    # generate actions
    for hrl in header_artifacts:
        _build_hrl(ctx, hrl, include_mapping[_header_key(hrl, name, is_private)])

    # construct updates build environment
    if not is_private:
        # fields for public include directory
        includes = _merge(include_mapping, build_environment.includes)
        private_includes = build_environment.private_includes
        include_dirs = _add(build_environment.include_dirs, name, anchor)
        private_include_dir = build_environment.private_include_dir
        app_includes = include_mapping
    else:
        # fields for private include directory
        includes = build_environment.includes
        private_includes = _merge(include_mapping, build_environment.private_includes)
        include_dirs = build_environment.include_dirs
        private_include_dir = [anchor] + build_environment.private_include_dir
        app_includes = build_environment.app_includes

    return BuildEnvironment(
        # updated fields
        includes = includes,
        private_includes = private_includes,
        include_dirs = include_dirs,
        private_include_dir = private_include_dir,
        deps_files = _merge(include_deps, build_environment.deps_files),
        app_includes = app_includes,
        # copied fields
        beams = build_environment.beams,
        priv_dirs = build_environment.priv_dirs,
        ebin_dirs = build_environment.ebin_dirs,
        app_beams = build_environment.app_beams,
        app_files = build_environment.app_files,
        full_dependencies = build_environment.full_dependencies,
        input_mapping = build_environment.input_mapping,
    )

def _header_key(hrl: "artifact", name: str, is_private: bool) -> [(str, str), str]:
    """Return the key for either public `("string", "string")` or private `"string"` include """
    return hrl.basename if is_private else (name, hrl.basename)

def _generate_beam_artifacts(
        ctx: AnalysisContext,
        toolchain: "Toolchain",
        build_environment: "BuildEnvironment",
        name: str,
        src_artifacts: list["artifact"],
        output_mapping: [None, dict["artifact", str]] = None) -> "BuildEnvironment":
    # anchor for ebin dir
    anchor = _make_dir_anchor(ctx, paths.join(_build_dir(toolchain), name, "ebin"))

    # output artifacts
    def output_path(src: "artifact") -> str:
        if output_mapping:
            return output_mapping[src]
        else:
            return beam_path(anchor, src.basename)

    beam_mapping = {
        module_name(src): ctx.actions.declare_output(output_path(src))
        for src in src_artifacts
    }

    _check_beam_uniqueness(beam_mapping, build_environment.beams)

    # dep files
    beam_deps = _get_deps_files(ctx, toolchain, anchor, src_artifacts, output_mapping)

    updated_build_environment = BuildEnvironment(
        # updated fields
        beams = _merge(beam_mapping, build_environment.beams),
        ebin_dirs = _add(build_environment.ebin_dirs, name, anchor),
        deps_files = _merge(beam_deps, build_environment.deps_files),
        app_beams = beam_mapping,
        # copied fields
        includes = build_environment.includes,
        private_includes = build_environment.private_includes,
        priv_dirs = build_environment.priv_dirs,
        include_dirs = build_environment.include_dirs,
        private_include_dir = build_environment.private_include_dir,
        app_includes = build_environment.app_includes,
        app_files = build_environment.app_files,
        full_dependencies = build_environment.full_dependencies,
        input_mapping = build_environment.input_mapping,
    )

    for erl in src_artifacts:
        _build_erl(ctx, toolchain, updated_build_environment, erl, beam_mapping[module_name(erl)])

    return updated_build_environment

def _check_beam_uniqueness(
        local_beams: ModuleArtifactMapping,
        global_beams: ModuleArtifactMapping) -> None:
    for module in local_beams:
        if module in global_beams:
            fail("duplicated modules found in build: {}".format([module]))
    return None

def _generate_chunk_artifacts(
        ctx: AnalysisContext,
        toolchain: "Toolchain",
        build_environment: "BuildEnvironment",
        name: str,
        src_artifacts: list["artifact"]) -> "BuildEnvironment":
    anchor = _make_dir_anchor(ctx, paths.join(_build_dir(toolchain), name, "chunks"))

    chunk_mapping = {
        module_name(src): ctx.actions.declare_output(chunk_path(anchor, src.basename))
        for src in src_artifacts
    }

    updated_build_environment = BuildEnvironment(
        app_chunks = chunk_mapping,
        # copied fields
        includes = build_environment.includes,
        private_includes = build_environment.private_includes,
        beams = build_environment.beams,
        priv_dirs = build_environment.priv_dirs,
        include_dirs = build_environment.include_dirs,
        private_include_dir = build_environment.private_include_dir,
        ebin_dirs = build_environment.ebin_dirs,
        deps_files = build_environment.deps_files,
        app_files = build_environment.app_files,
        full_dependencies = build_environment.full_dependencies,
        app_includes = build_environment.app_includes,
        app_beams = build_environment.app_beams,
        input_mapping = build_environment.input_mapping,
    )

    preprocess_modules = read_root_config("erlang", "edoc_preprocess", "").split()
    preprocess_all = "__all__" in preprocess_modules

    for erl in src_artifacts:
        preprocess = preprocess_all or module_name(erl) in preprocess_modules
        _build_edoc(ctx, toolchain, updated_build_environment, erl, chunk_mapping[module_name(erl)], preprocess)

    return updated_build_environment

def _make_dir_anchor(ctx: AnalysisContext, path: str) -> "artifact":
    return ctx.actions.write(
        paths.normalize(paths.join(path, ".hidden")),
        cmd_args([""]),
    )

def _get_deps_files(
        ctx: AnalysisContext,
        toolchain: "Toolchain",
        anchor: "artifact",
        srcs: list["artifact"],
        output_mapping: [None, dict["artifact", str]] = None) -> dict[str, "artifact"]:
    """Mapping from the output path to the deps file artifact for each srcs artifact."""

    def output_path(src: "artifact") -> str:
        return output_mapping[src] if output_mapping else _deps_key(anchor, src)

    return {
        output_path(src): _get_deps_file(ctx, toolchain, src)
        for src in srcs
    }

def _deps_key(anchor: "artifact", src: "artifact") -> str:
    name, ext = paths.split_extension(src.basename)
    if ext == ".erl":
        ext = ".beam"
    return anchor_path(anchor, name + ext)

def _get_deps_file(ctx: AnalysisContext, toolchain: "Toolchain", src: "artifact") -> "artifact":
    dependency_analyzer = toolchain.dependency_analyzer
    dependency_json = ctx.actions.declare_output(_dep_file_name(toolchain, src))
    escript = toolchain.otp_binaries.escript

    dependency_analyzer_cmd = cmd_args(
        [
            escript,
            dependency_analyzer,
            src,
            dependency_json.as_output(),
        ],
    )
    _run_with_env(
        ctx,
        toolchain,
        dependency_analyzer_cmd,
        category = "dependency_analyzer",
        identifier = action_identifier(toolchain, src.short_path),
    )
    return dependency_json

def _build_hrl(
        ctx: AnalysisContext,
        hrl: "artifact",
        output: "artifact") -> None:
    """Copy the header file and add dependencies on other includes."""
    ctx.actions.copy_file(output.as_output(), hrl)
    return None

def _build_xyrl(
        ctx: AnalysisContext,
        toolchain: "Toolchain",
        xyrl: "artifact",
        output: "artifact") -> "artifact":
    """Generate an erl file out of an xrl or yrl input file."""
    erlc = toolchain.otp_binaries.erlc
    erlc_cmd = cmd_args(
        [
            erlc,
            "-o",
            cmd_args(output.as_output()).parent(),
            xyrl,
        ],
    )
    _run_with_env(
        ctx,
        toolchain,
        erlc_cmd,
        category = "erlc",
        identifier = action_identifier(toolchain, xyrl.basename),
    )
    return output

def _build_erl(
        ctx: AnalysisContext,
        toolchain: "Toolchain",
        build_environment: "BuildEnvironment",
        src: "artifact",
        output: "artifact") -> None:
    """Compile erl files into beams."""

    trampoline = toolchain.erlc_trampoline
    erlc = toolchain.otp_binaries.erlc

    def dynamic_lambda(ctx: AnalysisContext, artifacts, outputs):
        erl_opts = _get_erl_opts(ctx, toolchain, src)
        erlc_cmd = cmd_args(
            [
                trampoline,
                erlc,
                erl_opts,
                _erlc_dependency_args(
                    _dependency_include_dirs(build_environment),
                    _dependency_code_paths(build_environment),
                ),
                "-o",
                cmd_args(outputs[output].as_output()).parent(),
                src,
            ],
        )
        erlc_cmd, mapping = _add_dependencies_to_args(ctx, artifacts, [outputs[output].short_path], {}, {}, erlc_cmd, build_environment)
        erlc_cmd = _add_full_dependencies(erlc_cmd, build_environment)
        _run_with_env(
            ctx,
            toolchain,
            erlc_cmd,
            category = "erlc",
            identifier = action_identifier(toolchain, src.basename),
            env = {"BUCK2_FILE_MAPPING": _generate_file_mapping_string(mapping)},
            always_print_stderr = True,
        )

    ctx.actions.dynamic_output(dynamic = build_environment.deps_files.values(), inputs = [src], outputs = [output], f = dynamic_lambda)
    return None

def _build_edoc(
        ctx: AnalysisContext,
        toolchain: "Toolchain",
        build_environment: "BuildEnvironment",
        src: "artifact",
        output: "artifact",
        preprocess: bool) -> None:
    """Build edoc from erl files."""
    eval_cmd = cmd_args(
        [
            toolchain.otp_binaries.escript,
            toolchain.edoc,
            cmd_args(toolchain.edoc_options),
            "-app",
            ctx.attrs.name,
            "-files",
            src,
            "-chunks",
            "-pa",
            toolchain.utility_modules,
            "-o",
            cmd_args(output.as_output()).parent(2),
        ],
    )

    if not preprocess:
        eval_cmd.add("-no-preprocess")

    args = _erlc_dependency_args(_dependency_include_dirs(build_environment), [], False)
    eval_cmd.add(args)

    for include in build_environment.includes.values():
        eval_cmd.hidden(include)

    for include in build_environment.private_includes.values():
        eval_cmd.hidden(include)

    _run_with_env(
        ctx,
        toolchain,
        eval_cmd,
        always_print_stderr = True,
        category = "edoc",
        identifier = action_identifier(toolchain, src.basename),
    )
    return None

def _add_dependencies_to_args(
        ctx: AnalysisContext,
        artifacts,
        queue: list[str],
        done: dict[str, bool],
        input_mapping: dict[str, (bool, [str, "artifact"])],
        args: cmd_args,
        build_environment: "BuildEnvironment") -> (cmd_args, dict[str, (bool, [str, "artifact"])]):
    """Add the transitive closure of all per-file Erlang dependencies as specified in the deps files to the `args` with .hidden.

    This function traverses the deps specified in the deps files and adds all discovered dependencies.
    """
    if not queue:
        return args, input_mapping

    next_round = []

    for key in queue:
        if key not in build_environment.deps_files:
            continue
        deps = artifacts[build_environment.deps_files[key]].read_json()

        # silently ignore not found dependencies and let erlc report the not found stuff
        for dep in deps:
            file = dep["file"]
            if dep["type"] == "include_lib":
                app = dep["app"]
                if (app, file) in build_environment.includes:
                    artifact = build_environment.includes[(app, file)]
                    input_mapping[file] = (True, build_environment.input_mapping[artifact.basename])
                else:
                    # the file might come from OTP
                    input_mapping[file] = (False, paths.join(app, "include", file))
                    continue

            elif dep["type"] == "include":
                # these includes can either reside in the private includes
                # or the public ones
                if file in build_environment.private_includes:
                    artifact = build_environment.private_includes[file]

                    if artifact.basename in build_environment.input_mapping:
                        input_mapping[file] = (True, build_environment.input_mapping[artifact.basename])
                else:
                    # at this point we don't know the application the include is coming
                    # from, and have to check all public include directories
                    candidates = [key for key in build_environment.includes.keys() if key[1] == file]
                    if len(candidates) > 1:
                        offending_apps = [app for (app, _) in candidates]
                        fail("-include(\"%s\") is ambiguous as the following applications declare public includes with the same name: %s" % (file, offending_apps))
                    elif candidates:
                        artifact = build_environment.includes[candidates[0]]
                        input_mapping[file] = (True, build_environment.input_mapping[artifact.basename])
                    else:
                        # we didn't find the include, build will fail during compile
                        continue

            elif (dep["type"] == "behaviour" or
                  dep["type"] == "parse_transform" or
                  dep["type"] == "manual_dependency"):
                module, _ = paths.split_extension(file)
                if module in build_environment.beams:
                    artifact = build_environment.beams[module]
                else:
                    continue

            else:
                fail("unrecognized dependency type %s", (dep["type"]))

            next_key = artifact.short_path
            if next_key not in done:
                done[next_key] = True
                next_round.append(next_key)
                args.hidden(artifact)

    # STARLARK does not have unbound loops (while loops) and we use recursion instead.
    return _add_dependencies_to_args(ctx, artifacts, next_round, done, input_mapping, args, build_environment)

def _add_full_dependencies(erlc_cmd: cmd_args, build_environment: "BuildEnvironment") -> cmd_args:
    for artifact in build_environment.full_dependencies:
        erlc_cmd.hidden(artifact)
    return erlc_cmd

def _dependency_include_dirs(build_environment: "BuildEnvironment") -> list[cmd_args]:
    includes = [
        cmd_args(include_dir_anchor).parent()
        for include_dir_anchor in build_environment.private_include_dir
    ]

    for include_dir_anchor in build_environment.include_dirs.values():
        includes.append(cmd_args(include_dir_anchor).parent(3))
        includes.append(cmd_args(include_dir_anchor).parent())

    return includes

def _dependency_code_paths(build_environment: "BuildEnvironment") -> list[cmd_args]:
    return [
        cmd_args(ebin_dir_anchor).parent()
        for ebin_dir_anchor in build_environment.ebin_dirs.values()
    ]

def _erlc_dependency_args(
        includes: list[cmd_args],
        code_paths: list[cmd_args],
        path_in_arg: bool = True) -> cmd_args:
    """Build include and path options."""
    # Q: why not just change format here - why do we add -I/-pa as a separate argument?
    # A: the whole string would get passed as a single argument, as if it was quoted in CLI e.g. '-I include_path'
    # ...which the escript cannot parse, as it expects two separate arguments, e.g. '-I' 'include_path'

    args = cmd_args([])

    # build -I options
    if path_in_arg:
        for include in includes:
            args.add(cmd_args(include, format = "-I{}"))
    else:
        for include in includes:
            args.add("-I")
            args.add(include)

    # build -pa options
    if path_in_arg:
        for code_path in code_paths:
            args.add(cmd_args(code_path, format = "-pa{}"))
    else:
        for code_path in code_paths:
            args.add("-pa")
            args.add(code_path)

    args.ignore_artifacts()

    return args

def _get_erl_opts(
        ctx: AnalysisContext,
        toolchain: "Toolchain",
        src: "artifact") -> cmd_args:
    always = ["+deterministic"]

    # use erl_opts defined in taret if present
    if getattr(ctx.attrs, "erl_opts", None) == None:
        opts = toolchain.erl_opts
    else:
        opts = ctx.attrs.erl_opts

    # dedupe options
    opts = dedupe(opts + always)

    # build args
    args = cmd_args(opts)

    # add parse_transforms
    parse_transforms = dict(toolchain.core_parse_transforms)
    if getattr(ctx.attrs, "use_global_parse_transforms", True):
        for parse_transform in toolchain.parse_transforms:
            if (
                # add parse_transform if there is no filter set
                not parse_transform in toolchain.parse_transforms_filters or
                # or if module is listed in the filter and add conditionally
                module_name(src) in toolchain.parse_transforms_filters[parse_transform]
            ):
                parse_transforms[parse_transform] = toolchain.parse_transforms[parse_transform]

    for parse_transform, (beam, resource_folder) in parse_transforms.items():
        args.add(
            "+{parse_transform, %s}" % (parse_transform,),
            cmd_args(beam, format = "-pa{}").parent(),
        )
        args.hidden(resource_folder)

    # add relevant compile_info manually
    args.add(cmd_args(
        src,
        format = "+{compile_info, [{source, \"{}\"}, {path_type, relative}, {options, []}]}",
    ))

    return args

def private_include_name(toolchain: "Toolchain", appname: str) -> str:
    """The temporary appname private header files."""
    return paths.join(
        _build_dir(toolchain),
        "__private_includes_%s" % (appname,),
    )

def generated_erl_path(toolchain: "Toolchain", appname: str, src: "artifact") -> str:
    """The output path for generated erl files."""
    return paths.join(
        _build_dir(toolchain),
        "__generated_%s" % (appname,),
        "%s.erl" % (module_name(src),),
    )

def anchor_path(anchor: "artifact", basename: str) -> str:
    """ Returns the output path for hrl files. """
    return paths.join(paths.dirname(anchor.short_path), basename)

def beam_path(anchor: "artifact", basename: str) -> str:
    """ Returns the output path for beam files. """
    return anchor_path(anchor, paths.replace_extension(basename, ".beam"))

def chunk_path(anchor: "artifact", basename: str) -> str:
    """Returns the output path for chunk files."""
    return anchor_path(anchor, paths.replace_extension(basename, ".chunk"))

def module_name(in_file: "artifact") -> str:
    """ Returns the basename of the artifact without extension """
    name, _ = paths.split_extension(in_file.basename)
    return name

def _is_hrl(in_file: "artifact") -> bool:
    """ Returns True if the artifact is a hrl file """
    return _is_ext(in_file, [".hrl"])

def _is_erl(in_file: "artifact") -> bool:
    """ Returns True if the artifact is an erl file """
    return _is_ext(in_file, [".erl"])

def _is_xyrl(in_file: "artifact") -> bool:
    """ Returns True if the artifact is a xrl or yrl file """
    return _is_ext(in_file, [".yrl", ".xrl"])

def _is_ext(in_file: "artifact", extensions: list[str]) -> bool:
    """ Returns True if the artifact has an extension listed in extensions """
    _, ext = paths.split_extension(in_file.basename)
    return ext in extensions

def _dep_file_name(toolchain: "Toolchain", src: "artifact") -> str:
    return paths.join(
        _build_dir(toolchain),
        "__dep_files",
        src.short_path + ".dep",
    )

def _merge(a: dict, b: dict) -> dict:
    """ sefely merge two dict """
    r = dict(a)
    r.update(b)
    return r

def _add(a: dict, key: "", value: "") -> dict:
    """ safely add a value to a dict """
    b = dict(a)
    b[key] = value
    return b

def _build_dir(toolchain: "Toolchain") -> str:
    return paths.join("__build", toolchain.name)

def _generate_file_mapping_string(mapping: dict[str, (bool, [str, "artifact"])]) -> cmd_args:
    """produces an easily parsable string for the file mapping"""
    items = {}
    for file, (if_found, artifact) in mapping.items():
        items[file] = (if_found, artifact)

    return to_term_args(items)

def _run_with_env(ctx: AnalysisContext, toolchain: "Toolchain", *args, **kwargs):
    """ run interfact that injects env"""

    # use os_env defined in target if present
    if getattr(ctx.attrs, "os_env", None) == None:
        env = toolchain.env
    else:
        env = ctx.attrs.os_env

    if "env" in kwargs:
        env = _merge(kwargs["env"], env)
    else:
        env = env
    kwargs["env"] = env
    ctx.actions.run(*args, **kwargs)

# export

erlang_build = struct(
    prepare_build_environment = _prepare_build_environment,
    build_steps = struct(
        generate_input_mapping = _generate_input_mapping,
        generated_source_artifacts = _generated_source_artifacts,
        generate_include_artifacts = _generate_include_artifacts,
        generate_beam_artifacts = _generate_beam_artifacts,
        generate_chunk_artifacts = _generate_chunk_artifacts,
    ),
    utils = struct(
        is_hrl = _is_hrl,
        is_erl = _is_erl,
        is_xyrl = _is_xyrl,
        module_name = module_name,
        private_include_name = private_include_name,
        make_dir_anchor = _make_dir_anchor,
        build_dir = _build_dir,
        run_with_env = _run_with_env,
    ),
)
