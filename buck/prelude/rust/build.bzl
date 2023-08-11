# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:local_only.bzl", "link_cxx_binary_locally")
load("@prelude//:paths.bzl", "paths")
load("@prelude//:resources.bzl", "create_resource_db", "gather_resources")
load("@prelude//cxx:cxx_context.bzl", "get_cxx_toolchain_info")
load("@prelude//cxx:cxx_library_utility.bzl", "cxx_attr_deps")
load(
    "@prelude//cxx:cxx_link_utility.bzl",
    "executable_shared_lib_arguments",
    "make_link_args",
)
load("@prelude//cxx:cxx_toolchain_types.bzl", "LinkerInfo")
load("@prelude//cxx:dwp.bzl", "dwp", "dwp_available")
load(
    "@prelude//cxx:linker.bzl",
    "get_default_shared_library_name",
    "get_shared_library_name_linker_flags",
)
load(
    "@prelude//linking:link_info.bzl",
    "LinkArgs",
    "LinkStyle",  #@unused Used as a type
    "get_link_args",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "merge_shared_libraries",
    "traverse_shared_library_info",
)
load("@prelude//os_lookup:defs.bzl", "OsLookup")
load("@prelude//utils:cmd_script.bzl", "ScriptOs", "cmd_script")
load("@prelude//utils:set.bzl", "set")
load("@prelude//utils:utils.bzl", "flatten_dict")
load(
    ":build_params.bzl",
    "BuildParams",  # @unused Used as a type
    "CrateType",
    "Emit",
    "crate_type_codegen",
    "crate_type_linked",
    "emit_needs_codegen",
    "output_filename",
)
load(":context.bzl", "CommonArgsInfo", "CompileContext")
load(":extern.bzl", "crate_map_arg", "extern_arg")
load(
    ":failure_filter.bzl",
    "RustFailureFilter",
    "failure_filter",
)
load(
    ":link_info.bzl",
    "CrateName",  #@unused Used as a type
    "RustLinkInfo",
    "RustLinkStyleInfo",
    "attr_crate",
    "attr_simple_crate_for_filenames",
    "inherited_non_rust_link_info",
    "inherited_non_rust_shared_libs",
    "normalize_crate",
    "resolve_deps",
    "style_info",
)
load(":resources.bzl", "rust_attr_resources")
load(":rust_toolchain.bzl", "RustToolchainInfo", "ctx_toolchain_info")

RustcOutput = record(
    output = field("artifact"),
    diag = field({str: "artifact"}),
    pdb = field(["artifact", None]),
    dwp_output = field(["artifact", None]),
)

def compile_context(ctx: AnalysisContext) -> CompileContext.type:
    toolchain_info = ctx_toolchain_info(ctx)
    cxx_toolchain_info = get_cxx_toolchain_info(ctx)

    # Setup source symlink tree.
    srcs = {src.short_path: src for src in ctx.attrs.srcs}
    srcs.update({k: v for v, k in ctx.attrs.mapped_srcs.items()})

    # Decide whether to use symlinked_dir or copied_dir.
    #
    # If a source is a prefix of any other source, use copied_dir. This supports
    # e.g. `srcs = [":foo.crate"]` where :foo.crate is an http_archive, together
    # with a `mapped_srcs` which overlays additional generated files into that
    # directory. Symlinked_dir would error in this situation.
    prefixes = {}
    symlinked_srcs = None
    for src in sorted(srcs.keys(), key = len, reverse = True):
        if src in prefixes:
            symlinked_srcs = ctx.actions.copied_dir("__srcs", srcs)
            break
        components = src.split("/")
        for i in range(1, len(components)):
            prefixes["/".join(components[:i])] = None
    if not symlinked_srcs:
        symlinked_srcs = ctx.actions.symlinked_dir("__srcs", srcs)

    linker = _linker_args(ctx, cxx_toolchain_info.linker_info)
    clippy_wrapper = _clippy_wrapper(ctx, toolchain_info)

    return CompileContext(
        toolchain_info = toolchain_info,
        cxx_toolchain_info = cxx_toolchain_info,
        symlinked_srcs = symlinked_srcs,
        linker_args = linker,
        clippy_wrapper = clippy_wrapper,
        common_args = {},
        flagfiles_for_extern = {},
        flagfiles_for_crate_map = {},
        transitive_dependency_dirs = {},
    )

def generate_rustdoc(
        ctx: AnalysisContext,
        compile_ctx: CompileContext.type,
        # link style doesn't matter, but caller should pass in build params
        # with static-pic (to get best cache hits for deps)
        params: BuildParams.type,
        default_roots: list[str],
        document_private_items: bool) -> "artifact":
    toolchain_info = compile_ctx.toolchain_info

    common_args = _compute_common_args(
        ctx = ctx,
        compile_ctx = compile_ctx,
        # to make sure we get the rmeta's generated for the crate dependencies,
        # rather than full .rlibs
        emit = Emit("metadata"),
        params = params,
        link_style = params.dep_link_style,
        default_roots = default_roots,
        is_rustdoc_test = False,
    )

    subdir = common_args.subdir + "-rustdoc"
    output = ctx.actions.declare_output(subdir)

    plain_env, path_env = _process_env(compile_ctx, ctx.attrs.env)

    rustdoc_cmd = cmd_args(
        [cmd_args("--env=", k, "=", v, delimiter = "") for k, v in plain_env.items()],
        [cmd_args("--path-env=", k, "=", v, delimiter = "") for k, v in path_env.items()],
        cmd_args(str(ctx.label.raw_target()), format = "--env=RUSTDOC_BUCK_TARGET={}"),
        toolchain_info.rustdoc,
        toolchain_info.rustdoc_flags,
        ctx.attrs.rustdoc_flags,
        common_args.args,
        cmd_args(output.as_output(), format = "--out-dir={}"),
    )

    if document_private_items:
        rustdoc_cmd.add("--document-private-items")

    url_prefix = toolchain_info.extern_html_root_url_prefix
    if url_prefix != None:
        # Flag --extern-html-root-url used below is only supported on nightly.
        rustdoc_cmd.add("-Zunstable-options")

        for rust_dependency in resolve_deps(ctx):
            dep = rust_dependency.dep
            if dep.label.cell != ctx.label.cell:
                # TODO: support a different extern_html_root_url_prefix per cell
                continue

            if rust_dependency.name:
                name = normalize_crate(rust_dependency.name)
            else:
                info = dep.get(RustLinkInfo)
                if info == None:
                    continue
                name = info.crate

            rustdoc_cmd.add(
                "--extern-html-root-url={}={}/{}:{}"
                    .format(name, url_prefix, dep.label.package, dep.label.name),
            )

    rustdoc_cmd.hidden(toolchain_info.rustdoc, compile_ctx.symlinked_srcs)

    rustdoc_cmd = _long_command(
        ctx = ctx,
        exe = toolchain_info.rustc_action,
        args = rustdoc_cmd,
        argfile_name = "{}.args".format(subdir),
    )

    ctx.actions.run(rustdoc_cmd, category = "rustdoc")

    return output

def generate_rustdoc_test(
        ctx: AnalysisContext,
        compile_ctx: CompileContext.type,
        link_style: LinkStyle.type,
        library: RustLinkStyleInfo.type,
        params: BuildParams.type,
        default_roots: list[str]) -> cmd_args:
    toolchain_info = compile_ctx.toolchain_info

    resources = create_resource_db(
        ctx = ctx,
        name = "doctest/resources.json",
        binary = library.rlib,
        resources = flatten_dict(gather_resources(
            label = ctx.label,
            resources = rust_attr_resources(ctx),
            deps = cxx_attr_deps(ctx),
        ).values()),
    )

    # Gather and setup symlink tree of transitive shared library deps.
    shared_libs = {}
    if link_style == LinkStyle("shared"):
        shlib_info = merge_shared_libraries(
            ctx.actions,
            deps = inherited_non_rust_shared_libs(ctx, include_doc_deps = True),
        )
        for soname, shared_lib in traverse_shared_library_info(shlib_info).items():
            shared_libs[soname] = shared_lib.lib
    extra_link_args, runtime_files, _ = executable_shared_lib_arguments(
        ctx.actions,
        compile_ctx.cxx_toolchain_info,
        resources,
        shared_libs,
    )

    common_args = _compute_common_args(
        ctx = ctx,
        compile_ctx = compile_ctx,
        emit = Emit("link"),
        params = params,
        link_style = params.dep_link_style,
        default_roots = default_roots,
        is_rustdoc_test = True,
        extra_transitive_deps = library.transitive_deps,
    )

    link_args, hidden, _pdb_artifact = make_link_args(
        ctx,
        [
            LinkArgs(flags = extra_link_args),
            get_link_args(
                inherited_non_rust_link_info(ctx, include_doc_deps = True),
                link_style,
            ),
        ],
        "{}-{}".format(common_args.subdir, common_args.tempfile),
    )

    link_args.add(ctx.attrs.doc_linker_flags or [])

    linker_argsfile, _ = ctx.actions.write(
        "{}/__{}_linker_args.txt".format(common_args.subdir, common_args.tempfile),
        link_args,
        allow_args = True,
    )

    if ctx.attrs._exec_os_type[OsLookup].platform == "windows":
        runtool = ["--runtool=cmd.exe", "--runtool-arg=/V:OFF", "--runtool-arg=/C"]
    else:
        runtool = ["--runtool=/usr/bin/env"]

    rustdoc_cmd = cmd_args(
        "--test",
        "-Zunstable-options",
        cmd_args("--test-builder=", toolchain_info.compiler, delimiter = ""),
        toolchain_info.rustdoc_flags,
        ctx.attrs.rustdoc_flags,
        common_args.args,
        extern_arg(ctx, compile_ctx, [], attr_crate(ctx), library.rlib),
        "--extern=proc_macro" if ctx.attrs.proc_macro else [],
        compile_ctx.linker_args,
        cmd_args(linker_argsfile, format = "-Clink-arg=@{}"),
        runtool,
        cmd_args(toolchain_info.rustdoc_test_with_resources, format = "--runtool-arg={}"),
        cmd_args("--runtool-arg=--resources=", resources, delimiter = ""),
        "--color=always",
        "--test-args=--color=always",
    )

    rustdoc_cmd.hidden(compile_ctx.symlinked_srcs, hidden, runtime_files)

    return _long_command(
        ctx = ctx,
        exe = toolchain_info.rustdoc,
        args = rustdoc_cmd,
        argfile_name = "{}.args".format(common_args.subdir),
    )

# Generate multiple compile artifacts so that distinct sets of artifacts can be
# generated concurrently.
def rust_compile_multi(
        ctx: AnalysisContext,
        compile_ctx: CompileContext.type,
        emits: list[Emit.type],
        params: BuildParams.type,
        link_style: LinkStyle.type,
        default_roots: list[str],
        extra_link_args: list[""] = [],
        predeclared_outputs: dict[Emit.type, "artifact"] = {},
        extra_flags: list[[str, "resolved_macro"]] = [],
        is_binary: bool = False) -> list[RustcOutput.type]:
    outputs = []

    for emit in emits:
        outs = rust_compile(
            ctx = ctx,
            compile_ctx = compile_ctx,
            emit = emit,
            params = params,
            link_style = link_style,
            default_roots = default_roots,
            extra_link_args = extra_link_args,
            predeclared_outputs = predeclared_outputs,
            extra_flags = extra_flags,
            is_binary = is_binary,
        )
        outputs.append(outs)

    return outputs

# Generate a compilation action. A single instance of rustc can emit
# numerous output artifacts, so return an artifact object for each of
# them.
def rust_compile(
        ctx: AnalysisContext,
        compile_ctx: CompileContext.type,
        emit: Emit.type,
        params: BuildParams.type,
        link_style: LinkStyle.type,
        default_roots: list[str],
        extra_link_args: list[""] = [],
        predeclared_outputs: dict[Emit.type, "artifact"] = {},
        extra_flags: list[[str, "resolved_macro"]] = [],
        is_binary: bool = False) -> RustcOutput.type:
    exec_is_windows = ctx.attrs._exec_os_type[OsLookup].platform == "windows"

    toolchain_info = compile_ctx.toolchain_info

    lints, clippy_lints = _lint_flags(compile_ctx)

    common_args = _compute_common_args(
        ctx = ctx,
        compile_ctx = compile_ctx,
        emit = emit,
        params = params,
        link_style = link_style,
        default_roots = default_roots,
        is_rustdoc_test = False,
    )

    rustc_cmd = cmd_args(
        # Lints go first to allow other args to override them.
        lints,
        # Report unused --extern crates in the notification stream.
        ["--json=unused-externs-silent", "-Wunused-crate-dependencies"] if toolchain_info.report_unused_deps else [],
        common_args.args,
        cmd_args("--remap-path-prefix=", compile_ctx.symlinked_srcs, "/=", cmd_args(ctx.label.path).replace_regex("\\\\", "/") if exec_is_windows else ctx.label.path, "/", delimiter = ""),
        compile_ctx.linker_args,
        extra_flags,
    )

    # If we're using failure filtering then we need to make sure the final
    # artifact location is the predeclared one since its specific path may have
    # already been encoded into the other compile args (eg rpath). So we still
    # let rustc_emit generate its own output artifacts, and then make sure we
    # use the predeclared one as the output after the failure filter action
    # below. Otherwise we'll use the predeclared outputs directly.
    if toolchain_info.failure_filter:
        emit_output, emit_args = _rustc_emit(
            ctx = ctx,
            compile_ctx = compile_ctx,
            emit = emit,
            predeclared_outputs = {},
            subdir = common_args.subdir,
            params = params,
        )
    else:
        emit_output, emit_args = _rustc_emit(
            ctx = ctx,
            compile_ctx = compile_ctx,
            emit = emit,
            predeclared_outputs = predeclared_outputs,
            subdir = common_args.subdir,
            params = params,
        )

    pdb_artifact = None
    dwp_inputs = []
    if crate_type_linked(params.crate_type) and not common_args.is_check:
        subdir = common_args.subdir
        tempfile = common_args.tempfile

        # If this crate type has an associated native dep link style, include deps
        # of that style.
        (link_args, hidden, pdb_artifact) = make_link_args(
            ctx,
            [
                LinkArgs(flags = extra_link_args),
                get_link_args(
                    inherited_non_rust_link_info(ctx),
                    link_style,
                ),
            ],
            "{}-{}".format(subdir, tempfile),
            output_short_path = emit_output.short_path,
        )
        linker_argsfile, _ = ctx.actions.write(
            "{}/__{}_linker_args.txt".format(subdir, tempfile),
            link_args,
            allow_args = True,
        )

        dwp_inputs = link_args
        rustc_cmd.add(cmd_args(linker_argsfile, format = "-Clink-arg=@{}"))
        rustc_cmd.hidden(hidden)

    (diag, build_status) = _rustc_invoke(
        ctx = ctx,
        compile_ctx = compile_ctx,
        prefix = "{}/{}".format(common_args.subdir, common_args.tempfile),
        rustc_cmd = cmd_args(toolchain_info.compiler, rustc_cmd, emit_args),
        diag = "diag",
        outputs = [emit_output],
        short_cmd = common_args.short_cmd,
        is_binary = is_binary,
        crate_map = common_args.crate_map,
    )

    # Add clippy diagnostic targets for check builds
    if common_args.is_check:
        # We don't really need the outputs from this build, just to keep the artifact accounting straight
        clippy_out, clippy_emit_args = _rustc_emit(
            ctx = ctx,
            compile_ctx = compile_ctx,
            emit = emit,
            predeclared_outputs = {},
            subdir = common_args.subdir + "-clippy",
            params = params,
        )
        clippy_env = dict()
        if toolchain_info.clippy_toml:
            # Clippy wants to be given a path to a directory containing a
            # clippy.toml (or .clippy.toml). Our buckconfig accepts an arbitrary
            # label like //path/to:my-clippy.toml which may not have the
            # filename that clippy looks for. Here we make a directory that
            # symlinks the requested configuration file under the required name.
            clippy_conf_dir = ctx.actions.symlinked_dir(
                common_args.subdir + "-clippy-configuration",
                {"clippy.toml": toolchain_info.clippy_toml},
            )
            clippy_env["CLIPPY_CONF_DIR"] = clippy_conf_dir
        (clippy_diag, _) = _rustc_invoke(
            ctx = ctx,
            compile_ctx = compile_ctx,
            prefix = "{}/{}".format(common_args.subdir, common_args.tempfile),
            # Lints go first to allow other args to override them.
            rustc_cmd = cmd_args(compile_ctx.clippy_wrapper, clippy_lints, rustc_cmd, clippy_emit_args),
            env = clippy_env,
            diag = "clippy",
            outputs = [clippy_out],
            short_cmd = common_args.short_cmd,
            is_binary = False,
            crate_map = common_args.crate_map,
        )
        diag.update(clippy_diag)

    if toolchain_info.failure_filter:
        # This is only needed when this action's output is being used as an
        # input, so we only need standard diagnostics (clippy is always
        # asked for explicitly).
        stderr = diag["diag.txt"]
        filter_prov = RustFailureFilter(
            buildstatus = build_status,
            required = emit_output,
            stderr = stderr,
        )

        filtered_output = failure_filter(
            ctx = ctx,
            compile_ctx = compile_ctx,
            prefix = "{}/{}".format(common_args.subdir, emit.value),
            predecl_out = predeclared_outputs.get(emit),
            failprov = filter_prov,
            short_cmd = common_args.short_cmd,
        )
    else:
        filtered_output = emit_output

    if is_binary and dwp_available(ctx):
        filtered_dwp_output = dwp(
            ctx,
            emit_output,
            identifier = "{}/__{}_{}_dwp".format(common_args.subdir, common_args.tempfile, str(emit)),
            category_suffix = "rust",
            # TODO(T110378142): Ideally, referenced objects are a list of
            # artifacts, but currently we don't track them properly.  So, we
            # just pass in the full link line and extract all inputs from that,
            # which is a bit of an overspecification.
            referenced_objects = dwp_inputs,
        )
    else:
        filtered_dwp_output = None

    return RustcOutput(
        output = filtered_output,
        diag = diag,
        pdb = pdb_artifact,
        dwp_output = filtered_dwp_output,
    )

# --extern <crate>=<path> for direct dependencies
# -Ldependency=<dir> for transitive dependencies
# For native dependencies, we use -Clink-arg=@argsfile
# Second element of result tuple is a list of files/directories that should be present for executable to be run successfully
# Third return is the mapping from crate names back to targets (needed so that a deps linter knows what deps need fixing)
def _dependency_args(
        ctx: AnalysisContext,
        compile_ctx: CompileContext.type,
        subdir: str,
        crate_type: CrateType.type,
        link_style: LinkStyle.type,
        is_check: bool,
        is_rustdoc_test: bool,
        extra_transitive_deps: dict["artifact", CrateName.type]) -> (cmd_args, list[(CrateName.type, Label)]):
    args = cmd_args()
    transitive_deps = {}
    deps = []
    crate_targets = []
    for x in resolve_deps(ctx, include_doc_deps = is_rustdoc_test):
        dep = x.dep

        deps.append(dep)

        # Rust dependency
        info = dep.get(RustLinkInfo)
        if info == None:
            continue
        if x.name:
            crate = CrateName(
                simple = normalize_crate(x.name),
                dynamic = None,
            )
        else:
            crate = info.crate

        style = style_info(info, link_style)

        # Use rmeta dependencies whenever possible because they
        # should be cheaper to produce.
        if is_check or (compile_ctx.toolchain_info.pipelined and not crate_type_codegen(crate_type) and not is_rustdoc_test):
            artifact = style.rmeta
            transitive_artifacts = style.transitive_rmeta_deps
        else:
            artifact = style.rlib
            transitive_artifacts = style.transitive_deps

        args.add(extern_arg(ctx, compile_ctx, x.flags, crate, artifact))
        crate_targets.append((crate, dep.label))

        # Unwanted transitive_deps have already been excluded
        transitive_deps.update(transitive_artifacts)

    transitive_deps.update(extra_transitive_deps)

    dynamic_artifacts = {}
    simple_artifacts = {}
    for artifact, crate_name in transitive_deps.items():
        if crate_name.dynamic:
            dynamic_artifacts[artifact] = crate_name
        else:
            simple_artifacts[artifact] = None

    prefix = "{}-deps{}".format(subdir, "-check" if is_check else "")
    if simple_artifacts:
        args.add(simple_symlinked_dirs(ctx, prefix, simple_artifacts))
    if dynamic_artifacts:
        args.add(dynamic_symlinked_dirs(ctx, compile_ctx, prefix, dynamic_artifacts))

    return (args, crate_targets)

def simple_symlinked_dirs(
        ctx: AnalysisContext,
        prefix: str,
        artifacts: dict["artifact", None]) -> cmd_args:
    # Add as many -Ldependency dirs as we need to avoid name conflicts
    deps_dirs = [{}]
    for dep in artifacts.keys():
        name = dep.basename
        if name in deps_dirs[-1]:
            deps_dirs.append({})
        deps_dirs[-1][name] = dep

    symlinked_dirs = []
    for idx, srcs in enumerate(deps_dirs):
        name = "{}-{}".format(prefix, idx)
        symlinked_dirs.append(ctx.actions.symlinked_dir(name, srcs))

    return cmd_args(symlinked_dirs, format = "-Ldependency={}")

def dynamic_symlinked_dirs(
        ctx: AnalysisContext,
        compile_ctx: CompileContext.type,
        prefix: str,
        artifacts: dict["artifact", CrateName.type]) -> cmd_args:
    name = "{}-dyn".format(prefix)
    transitive_dependency_dir = ctx.actions.declare_output(name, dir = True)
    do_symlinks = cmd_args(
        compile_ctx.toolchain_info.transitive_dependency_symlinks_tool,
        cmd_args(transitive_dependency_dir.as_output(), format = "--out-dir={}"),
    )
    for artifact, crate in artifacts.items():
        relative_path = cmd_args(artifact).relative_to(transitive_dependency_dir.project("i"))
        do_symlinks.add("--artifact", crate.dynamic, relative_path.ignore_artifacts())
    ctx.actions.run(
        do_symlinks,
        category = "tdep_symlinks",
        identifier = str(len(compile_ctx.transitive_dependency_dirs)),
    )
    compile_ctx.transitive_dependency_dirs[transitive_dependency_dir] = None
    return cmd_args(transitive_dependency_dir, format = "@{}/dirs").hidden(artifacts.keys())

def _lintify(flag: str, clippy: bool, lints: list["resolved_macro"]) -> cmd_args:
    return cmd_args(
        [lint for lint in lints if str(lint).startswith("\"clippy::") == clippy],
        format = "-{}{{}}".format(flag),
    )

def _lint_flags(compile_ctx: CompileContext.type) -> (cmd_args, cmd_args):
    toolchain_info = compile_ctx.toolchain_info

    plain = cmd_args(
        _lintify("A", False, toolchain_info.allow_lints),
        _lintify("D", False, toolchain_info.deny_lints),
        _lintify("W", False, toolchain_info.warn_lints),
    )

    clippy = cmd_args(
        _lintify("A", True, toolchain_info.allow_lints),
        _lintify("D", True, toolchain_info.deny_lints),
        _lintify("W", True, toolchain_info.warn_lints),
    )

    return (plain, clippy)

def _rustc_flags(flags: list[[str, "resolved_macro"]]) -> list[[str, "resolved_macro"]]:
    # Rustc's "-g" flag is documented as being exactly equivalent to
    # "-Cdebuginfo=2". Rustdoc supports the latter, it just doesn't have the
    # "-g" shorthand for it.
    for i, flag in enumerate(flags):
        if str(flag) == '"-g"':
            flags[i] = "-Cdebuginfo=2"

    return flags

# Compute which are common to both rustc and rustdoc
def _compute_common_args(
        ctx: AnalysisContext,
        compile_ctx: CompileContext.type,
        emit: Emit.type,
        params: BuildParams.type,
        link_style: LinkStyle.type,
        default_roots: list[str],
        is_rustdoc_test: bool,
        extra_transitive_deps: dict["artifact", CrateName.type] = {}) -> CommonArgsInfo.type:
    crate_type = params.crate_type

    args_key = (crate_type, emit, link_style, is_rustdoc_test)
    if args_key in compile_ctx.common_args:
        return compile_ctx.common_args[args_key]

    # Keep filenames distinct in per-flavour subdirs
    subdir = "{}-{}-{}-{}".format(crate_type.value, params.reloc_model.value, link_style.value, emit.value)
    if is_rustdoc_test:
        subdir = "{}-rustdoc-test".format(subdir)

    # Included in tempfiles
    tempfile = "{}-{}".format(attr_simple_crate_for_filenames(ctx), emit.value)

    srcs = ctx.attrs.srcs
    mapped_srcs = ctx.attrs.mapped_srcs
    all_srcs = map(lambda s: s.short_path, srcs) + mapped_srcs.values()
    crate_root = ctx.attrs.crate_root or _crate_root(ctx, all_srcs, default_roots)

    is_check = not emit_needs_codegen(emit)

    dependency_args, crate_map = _dependency_args(
        ctx = ctx,
        compile_ctx = compile_ctx,
        subdir = subdir,
        crate_type = crate_type,
        link_style = link_style,
        is_check = is_check,
        is_rustdoc_test = is_rustdoc_test,
        extra_transitive_deps = extra_transitive_deps,
    )

    if crate_type == CrateType("proc-macro"):
        dependency_args.add("--extern=proc_macro")

    if crate_type == CrateType("cdylib") and not is_check:
        linker_info = compile_ctx.cxx_toolchain_info.linker_info
        shlib_name = get_default_shared_library_name(linker_info, ctx.label)
        dependency_args.add(cmd_args(
            get_shared_library_name_linker_flags(linker_info.type, shlib_name),
            format = "-Clink-arg={}",
        ))

    toolchain_info = compile_ctx.toolchain_info
    edition = ctx.attrs.edition or toolchain_info.default_edition or \
              fail("missing 'edition' attribute, and there is no 'default_edition' set by the toolchain")

    crate = attr_crate(ctx)
    if crate.dynamic:
        crate_name_arg = cmd_args("--crate-name", cmd_args("@", crate.dynamic, delimiter = ""))
    else:
        crate_name_arg = cmd_args("--crate-name=", crate.simple, delimiter = "")

    args = cmd_args(
        cmd_args(compile_ctx.symlinked_srcs, "/", crate_root, delimiter = ""),
        crate_name_arg,
        "--crate-type={}".format(crate_type.value),
        "-Crelocation-model={}".format(params.reloc_model.value),
        "--edition={}".format(edition),
        "-Cmetadata={}".format(_metadata(ctx.label)[0]),
        # Make diagnostics json with the option to extract rendered text
        ["--error-format=json", "--json=diagnostic-rendered-ansi"] if not is_rustdoc_test else [],
        ["-Cprefer-dynamic=yes"] if crate_type == CrateType("dylib") else [],
        ["--target={}".format(toolchain_info.rustc_target_triple)] if toolchain_info.rustc_target_triple else [],
        _rustc_flags(toolchain_info.rustc_flags),
        _rustc_flags(toolchain_info.rustc_check_flags) if is_check else [],
        _rustc_flags(toolchain_info.rustc_coverage_flags) if ctx.attrs.coverage else [],
        _rustc_flags(ctx.attrs.rustc_flags),
        cmd_args(ctx.attrs.features, format = '--cfg=feature="{}"'),
        dependency_args,
    )

    common_args = CommonArgsInfo(
        args = args,
        subdir = subdir,
        tempfile = tempfile,
        short_cmd = "{},{},{}".format(crate_type.value, params.reloc_model.value, emit.value),
        is_check = is_check,
        crate_map = crate_map,
    )

    compile_ctx.common_args[args_key] = common_args
    return common_args

# Return wrapper script for clippy-driver to make sure sysroot is set right
# We need to make sure clippy is using the same sysroot - compiler, std libraries -
# as rustc itself, so explicitly invoke rustc to get the path. This is a
# (small - ~15ms per invocation) perf hit but only applies when generating
# specifically requested clippy diagnostics.
def _clippy_wrapper(
        ctx: AnalysisContext,
        toolchain_info: RustToolchainInfo.type) -> cmd_args:
    clippy_driver = cmd_args(toolchain_info.clippy_driver)
    rustc_print_sysroot = cmd_args(toolchain_info.compiler, "--print=sysroot", delimiter = " ")
    if toolchain_info.rustc_target_triple:
        rustc_print_sysroot.add("--target={}".format(toolchain_info.rustc_target_triple))

    if ctx.attrs._exec_os_type[OsLookup].platform == "windows":
        wrapper_file, _ = ctx.actions.write(
            ctx.actions.declare_output("__clippy_driver_wrapper.bat"),
            [
                "@echo off",
                "set __CLIPPY_INTERNAL_TESTS=true",
                cmd_args(rustc_print_sysroot, format = 'FOR /F "tokens=* USEBACKQ" %%F IN (`{}`) DO ('),
                "set SYSROOT=%%F",
                ")",
                cmd_args(clippy_driver, format = "{} %*"),
            ],
            allow_args = True,
        )
    else:
        wrapper_file, _ = ctx.actions.write(
            ctx.actions.declare_output("__clippy_driver_wrapper.sh"),
            [
                "#!/usr/bin/env bash",
                # Force clippy to be clippy: https://github.com/rust-lang/rust-clippy/blob/e405c68b3c1265daa9a091ed9b4b5c5a38c0c0ba/src/driver.rs#L334
                "export __CLIPPY_INTERNAL_TESTS=true",
                cmd_args(rustc_print_sysroot, format = "export SYSROOT=$({})"),
                cmd_args(clippy_driver, format = "{} \"$@\"\n"),
            ],
            is_executable = True,
            allow_args = True,
        )

    return cmd_args(wrapper_file).hidden(clippy_driver, rustc_print_sysroot)

# This is a hack because we need to pass the linker to rustc
# using -Clinker=path and there is currently no way of doing this
# without an artifact. We create a wrapper (which is an artifact),
# and add -Clinker=
def _linker_args(
        ctx: AnalysisContext,
        linker_info: LinkerInfo.type) -> cmd_args:
    linker = cmd_args(
        linker_info.linker,
        linker_info.linker_flags or [],
        ctx.attrs.linker_flags,
    )

    linker_wrapper = cmd_script(
        ctx = ctx,
        name = "linker_wrapper",
        cmd = linker,
        os = ScriptOs("windows" if ctx.attrs._exec_os_type[OsLookup].platform == "windows" else "unix"),
    )

    return cmd_args(linker_wrapper, format = "-Clinker={}")

# Returns the full label and its hash. The full label is used for `-Cmetadata`
# which provided the primary disambiguator for two otherwise identically named
# crates. The hash is added to the filename to give them a lower likelihood of
# duplicate names, but it doesn't matter if they collide.
def _metadata(label: Label) -> (str, str):
    label = str(label.raw_target())
    h = hash(label)
    if h < 0:
        h = -h
    h = "%x" % h
    return (label, "0" * (8 - len(h)) + h)

def _crate_root(
        ctx: AnalysisContext,
        srcs: list[str],
        default_roots: list[str]) -> str:
    candidates = set()
    if getattr(ctx.attrs, "crate_dynamic", None):
        crate_with_suffix = None
    else:
        crate_with_suffix = attr_crate(ctx).simple + ".rs"
    for src in srcs:
        filename = src.split("/")[-1]
        if filename in default_roots or filename == crate_with_suffix:
            candidates.add(src)

    if candidates.size() == 1:
        return candidates.list()[0]

    fail("Could not infer crate_root. candidates=%s\nAdd 'crate_root = \"src/example.rs\"' to your attributes to disambiguate." % candidates.list())

# Take a desired output and work out how to convince rustc to generate it
def _rustc_emit(
        ctx: AnalysisContext,
        compile_ctx: CompileContext.type,
        emit: Emit.type,
        predeclared_outputs: dict[Emit.type, "artifact"],
        subdir: str,
        params: BuildParams.type) -> ("artifact", cmd_args):
    toolchain_info = compile_ctx.toolchain_info
    simple_crate = attr_simple_crate_for_filenames(ctx)
    crate_type = params.crate_type

    # Metadata for pipelining needs has enough info to be used as an input
    # for dependents. To do this reliably, we actually emit "link" but
    # suppress actual codegen with -Zno-codegen.
    #
    # We don't bother to do this with "codegen" crates - ie, ones which are
    # linked into an artifact like binaries and dylib, since they're not
    # used as a pipelined dependency input.
    pipeline_meta = emit == Emit("metadata") and \
                    toolchain_info.pipelined and \
                    not crate_type_codegen(crate_type)

    emit_args = cmd_args()
    if emit in predeclared_outputs:
        emit_output = predeclared_outputs[emit]
    else:
        extra_hash = "-" + _metadata(ctx.label)[1]
        emit_args.add("-Cextra-filename={}".format(extra_hash))
        if pipeline_meta:
            # Make sure hollow rlibs are distinct from real ones
            filename = subdir + "/hollow/" + output_filename(simple_crate, Emit("link"), params, extra_hash)
        else:
            filename = subdir + "/" + output_filename(simple_crate, emit, params, extra_hash)

        emit_output = ctx.actions.declare_output(filename)

    if pipeline_meta:
        # If we're doing a pipelined build, instead of emitting an actual rmeta
        # we emit a "hollow" .rlib - ie, it only contains lib.rmeta and no object
        # code. It should contain full information needed by any dependent
        # crate which is generating code (MIR, etc).
        # Requires https://github.com/rust-lang/rust/pull/86045
        emit_args.add(
            cmd_args(emit_output.as_output(), format = "--emit=link={}"),
            "-Zno-codegen",
        )
    elif emit == Emit("expand"):
        emit_args.add(
            "-Zunpretty=expanded",
            cmd_args(emit_output.as_output(), format = "-o{}"),
        )
    else:
        # Assume https://github.com/rust-lang/rust/issues/85356 is fixed (ie
        # https://github.com/rust-lang/rust/pull/85362 is applied)
        emit_args.add(cmd_args("--emit=", emit.value, "=", emit_output.as_output(), delimiter = ""))

    if emit != Emit("expand"):
        # Strip file extension from directory name.
        base, _ext = paths.split_extension(output_filename(simple_crate, emit, params))
        extra_dir = subdir + "/extras/" + base
        extra_out = ctx.actions.declare_output(extra_dir, dir = True)
        emit_args.add(cmd_args(extra_out.as_output(), format = "--out-dir={}"))

        if ctx.attrs.incremental_enabled:
            build_mode = ctx.attrs.incremental_build_mode
            incremental_out = ctx.actions.declare_output("{}/extras/incremental/{}".format(subdir, build_mode))
            incremental_cmd = cmd_args(incremental_out.as_output(), format = "-Cincremental={}")
            emit_args.add(incremental_cmd)

    return (emit_output, emit_args)

# Invoke rustc and capture outputs
def _rustc_invoke(
        ctx: AnalysisContext,
        compile_ctx: CompileContext.type,
        prefix: str,
        rustc_cmd: cmd_args,
        diag: str,
        outputs: list["artifact"],
        short_cmd: str,
        is_binary: bool,
        crate_map: list[(CrateName.type, Label)],
        env: dict[str, ["resolved_macro", "artifact"]] = {}) -> (dict[str, "artifact"], ["artifact", None]):
    toolchain_info = compile_ctx.toolchain_info

    plain_env, path_env = _process_env(compile_ctx, ctx.attrs.env)

    more_plain_env, more_path_env = _process_env(compile_ctx, env)
    plain_env.update(more_plain_env)
    path_env.update(more_path_env)

    # Save diagnostic outputs
    json_diag = ctx.actions.declare_output("{}-{}.json".format(prefix, diag))
    txt_diag = ctx.actions.declare_output("{}-{}.txt".format(prefix, diag))

    compile_cmd = cmd_args(
        cmd_args(json_diag.as_output(), format = "--diag-json={}"),
        cmd_args(txt_diag.as_output(), format = "--diag-txt={}"),
        "--remap-cwd-prefix=",
        "--buck-target={}".format(ctx.label.raw_target()),
    )

    for k, v in crate_map:
        compile_cmd.add(crate_map_arg(ctx, compile_ctx, k, v))
    for k, v in plain_env.items():
        compile_cmd.add(cmd_args("--env=", k, "=", v, delimiter = ""))
    for k, v in path_env.items():
        compile_cmd.add(cmd_args("--path-env=", k, "=", v, delimiter = ""))

    build_status = None
    if toolchain_info.failure_filter:
        # Build status for fail filter
        build_status = ctx.actions.declare_output("{}_build_status-{}.json".format(prefix, diag))
        compile_cmd.add(cmd_args(build_status.as_output(), format = "--failure-filter={}"))
        for out in outputs:
            compile_cmd.add("--required-output", out.short_path, out.as_output())

    compile_cmd.add(rustc_cmd)
    compile_cmd.hidden(toolchain_info.compiler, compile_ctx.symlinked_srcs)

    compile_cmd = _long_command(
        ctx = ctx,
        exe = toolchain_info.rustc_action,
        args = compile_cmd,
        argfile_name = "{}-{}.args".format(prefix, diag),
    )

    incremental_enabled = ctx.attrs.incremental_enabled
    local_only = False
    prefer_local = False
    if incremental_enabled:
        local_only = True
    elif is_binary and link_cxx_binary_locally(ctx):
        prefer_local = True

    identifier = "{} {} [{}]".format(prefix, short_cmd, diag)
    ctx.actions.run(
        compile_cmd,
        local_only = local_only,
        prefer_local = prefer_local,
        category = "rustc",
        identifier = identifier,
        no_outputs_cleanup = incremental_enabled,
    )

    return ({diag + ".json": json_diag, diag + ".txt": txt_diag}, build_status)

# Our rustc and rustdoc commands can have arbitrarily large number of `--extern`
# flags, so write to file to avoid hitting the platform's limit on command line
# length. This limit is particularly small on Windows.
def _long_command(
        ctx: AnalysisContext,
        exe: RunInfo.type,
        args: cmd_args,
        argfile_name: str) -> cmd_args:
    argfile, hidden = ctx.actions.write(argfile_name, args, allow_args = True)
    return cmd_args(exe, cmd_args(argfile, format = "@{}")).hidden(args, hidden)

# Separate env settings into "plain" and "with path". Path env vars are often
# used in Rust `include!()` and similar directives, which always interpret the
# path relative to the source file containing the directive. Since paths in env
# vars are often expanded from macros such as `$(location)`, they will be
# cell-relative which will not work properly. To solve this, we canonicalize
# paths to absolute paths so they'll work in any context. Hence the need to
# distinguish path from non-path. (This will not work if the value contains both
# path and non-path content, but we'll burn that bridge when we get to it.)
def _process_env(
        compile_ctx: CompileContext.type,
        env: dict[str, ["resolved_macro", "artifact"]]) -> (dict[str, cmd_args], dict[str, cmd_args]):
    # Values with inputs (ie artifact references).
    path_env = {}

    # Plain strings.
    plain_env = {}

    for k, v in env.items():
        v = cmd_args(v)
        if len(v.inputs) > 0:
            path_env[k] = v
        else:
            # Environment variables may have newlines, escape them for now.
            # Will be unescaped in rustc_action.
            # Variable may have "\\n" as well.
            # Example: \\n\n -> \\\n\n -> \\\\n\\n
            plain_env[k] = v.replace_regex("\\\\n", "\\\n").replace_regex("\\n", "\\n")

    # If CARGO_MANIFEST_DIR is not already expressed in terms of $(location ...)
    # of some target, then interpret it as a relative path inside of the crate's
    # sources.
    #
    # For example in the following case:
    #
    #     http_archive(
    #         name = "foo.crate",
    #         ...
    #     )
    #
    #     rust_library(
    #         name = "foo",
    #         srcs = [":foo.crate"],
    #         crate_root = "foo.crate/src/lib.rs",
    #         env = {
    #             "CARGO_MANIFEST_DIR": "foo.crate",
    #         },
    #     )
    #
    # then the manifest directory refers to the directory which is the parent of
    # `src` inside the archive.
    #
    # By putting the environment variable into path_env, rustc_action.py will
    # take care of turning this into an absolute path before rustc sees it. This
    # matches Cargo which also always provides CARGO_MANIFEST_DIR as an absolute
    # path. A relative path would be problematic because it can't simultaneously
    # support both of the following real-world cases: `include!` which resolves
    # relative paths relative to the file containing the include:
    #
    #     include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/thing.rs"));
    #
    # and proc macros using std::fs to read thing like .pest grammars, which
    # would need paths relative to the directory that rustc got invoked in
    # (which is the repo root in Buck builds).
    cargo_manifest_dir = plain_env.pop("CARGO_MANIFEST_DIR", None)
    if cargo_manifest_dir:
        path_env["CARGO_MANIFEST_DIR"] = cmd_args(
            compile_ctx.symlinked_srcs,
            "/",
            cargo_manifest_dir,
            delimiter = "",
        )

    return (plain_env, path_env)
