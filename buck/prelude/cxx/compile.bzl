# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:paths.bzl", "paths")
load("@prelude//linking:lto.bzl", "LtoMode")
load(
    "@prelude//utils:utils.bzl",
    "flatten",
)
load(":argsfiles.bzl", "CompileArgsfile", "CompileArgsfiles")
load(":attr_selection.bzl", "cxx_by_language_ext")
load(
    ":compiler.bzl",
    "get_flags_for_colorful_output",
    "get_flags_for_reproducible_build",
    "get_headers_dep_files_flags_factory",
    "get_output_flags",
    "get_pic_flags",
)
load(":cxx_context.bzl", "get_cxx_toolchain_info")
load(":cxx_toolchain_types.bzl", "CxxObjectFormat", "DepTrackingMode")
load(":debug.bzl", "SplitDebugMode")
load(
    ":headers.bzl",
    "CPrecompiledHeaderInfo",
)
load(":platform.bzl", "cxx_by_platform")
load(
    ":preprocessor.bzl",
    "CPreprocessor",  # @unused Used as a type
    "CPreprocessorInfo",  # @unused Used as a type
    "cxx_attr_preprocessor_flags",
    "cxx_merge_cpreprocessors",
    "get_flags_for_compiler_type",
)

# Supported Cxx file extensions
CxxExtension = enum(
    ".cpp",
    ".cc",
    ".cxx",
    ".c++",
    ".c",
    ".s",
    ".S",
    ".m",
    ".mm",
    ".cu",
    ".hip",
    ".asm",
    ".asmpp",
    ".h",
    ".hpp",
)

# File types for dep files
DepFileType = enum(
    "cpp",
    "c",
    "cuda",
    "asm",
)

_HeadersDepFiles = record(
    # An executable to wrap the actual command with for post-processing of dep
    # files into the format that Buck2 recognizes (i.e. one artifact per line).
    processor = field(cmd_args),
    # The tag that was added to headers.
    tag = field("artifact_tag"),
    # A function that produces new cmd_args to append to the compile command to
    # get it to emit the dep file. This will receive the output dep file as an
    # input.
    mk_flags = field("function"),
    # Dependency tracking mode to know how to generate dep file
    dep_tracking_mode = field(DepTrackingMode.type),
)

# Information about how to compile a source file of particular extension.
_CxxCompileCommand = record(
    # The compiler and any args which are independent of the rule.
    base_compile_cmd = field(cmd_args),
    # The argsfile of arguments from the rule and it's dependencies.
    argsfile = field(CompileArgsfile.type),
    headers_dep_files = field([_HeadersDepFiles.type, None]),
    compiler_type = field(str),
)

# Information about how to compile a source file.
CxxSrcCompileCommand = record(
    # Source file to compile.
    src = field("artifact"),
    # If we have multiple source entries with same files but different flags,
    # specify an index so we can differentiate them. Otherwise, use None.
    index = field(["int", None], None),
    # The CxxCompileCommand to use to compile this file.
    cxx_compile_cmd = field(_CxxCompileCommand.type),
    # Arguments specific to the source file.
    args = field(["_arg"]),
)

# Output of creating compile commands for Cxx source files.
CxxCompileCommandOutput = record(
    # List of compile commands for each source file.
    src_compile_cmds = field([CxxSrcCompileCommand.type], default = []),
    # Argsfiles generated for compiling these source files.
    argsfiles = field(CompileArgsfiles.type, default = CompileArgsfiles()),
    # List of compile commands for use in compilation database generation.
    comp_db_compile_cmds = field([CxxSrcCompileCommand.type], default = []),
)

# An input to cxx compilation, consisting of a file to compile and optional
# file specific flags to compile with.
CxxSrcWithFlags = record(
    file = field("artifact"),
    flags = field(["resolved_macro"], []),
    # If we have multiple source entries with same files but different flags,
    # specify an index so we can differentiate them. Otherwise, use None.
    index = field(["int", None], None),
)

CxxCompileOutput = record(
    # The compiled `.o` file.
    object = field("artifact"),
    object_format = field(CxxObjectFormat.type, CxxObjectFormat("native")),
    object_has_external_debug_info = field(bool, False),
    # Externally referenced debug info, which doesn't get linked with the
    # object (e.g. the above `.o` when using `-gsplit-dwarf=single` or the
    # the `.dwo` when using `-gsplit-dwarf=split`).
    external_debug_info = field(["artifact", None], None),
    clang_remarks = field(["artifact", None], None),
    clang_trace = field(["artifact", None], None),
)

def create_compile_cmds(
        ctx: AnalysisContext,
        impl_params: "CxxRuleConstructorParams",
        own_preprocessors: list[CPreprocessor.type],
        inherited_preprocessor_infos: list[CPreprocessorInfo.type],
        absolute_path_prefix: [str, None]) -> CxxCompileCommandOutput.type:
    """
    Forms the CxxSrcCompileCommand to use for each source file based on it's extension
    and optional source file flags. Returns CxxCompileCommandOutput containing an array
    of the generated compile commands and argsfile output.
    """

    srcs_with_flags = []
    for src in impl_params.srcs:
        srcs_with_flags.append(src)
    header_only = False
    if len(srcs_with_flags) == 0 and len(impl_params.additional.srcs) == 0:
        all_headers = flatten([x.headers for x in own_preprocessors])
        if len(all_headers) == 0:
            all_raw_headers = flatten([x.raw_headers for x in own_preprocessors])
            if len(all_raw_headers) != 0:
                header_only = True
                for header in all_raw_headers:
                    if header.extension in [".h", ".hpp"]:
                        srcs_with_flags.append(CxxSrcWithFlags(file = header))
            else:
                return CxxCompileCommandOutput()
        else:
            header_only = True
            for header in all_headers:
                if header.artifact.extension in [".h", ".hpp", ".cpp"]:
                    srcs_with_flags.append(CxxSrcWithFlags(file = header.artifact))

    # TODO(T110378129): Buck v1 validates *all* headers used by a compilation
    # at compile time, but that doing that here/eagerly might be expensive (but
    # we should figure out something).
    _validate_target_headers(ctx, own_preprocessors)

    # Combine all preprocessor info and prepare it for compilations.
    pre = cxx_merge_cpreprocessors(
        ctx,
        filter(None, own_preprocessors + impl_params.extra_preprocessors),
        inherited_preprocessor_infos,
    )

    headers_tag = ctx.actions.artifact_tag()
    abs_headers_tag = ctx.actions.artifact_tag()  # This headers tag is just for convenience use in _mk_argsfile and is otherwise unused.

    src_compile_cmds = []
    cxx_compile_cmd_by_ext = {}
    argsfile_by_ext = {}
    abs_argsfile_by_ext = {}

    for src in srcs_with_flags:
        # If we have a header_only library we'll send the header files through this path,
        # and want them to appear as though they are C++ files.
        ext = CxxExtension(".cpp" if header_only else src.file.extension)

        # Deduplicate shared arguments to save memory. If we compile multiple files
        # of the same extension they will have some of the same flags. Save on
        # allocations by caching and reusing these objects.
        if not ext in cxx_compile_cmd_by_ext:
            toolchain = get_cxx_toolchain_info(ctx)
            compiler_info = _get_compiler_info(toolchain, ext)
            base_compile_cmd = _get_compile_base(compiler_info)

            headers_dep_files = None
            dep_file_file_type_hint = _dep_file_type(ext)
            if dep_file_file_type_hint != None and toolchain.use_dep_files:
                tracking_mode = _get_dep_tracking_mode(toolchain, dep_file_file_type_hint)
                mk_dep_files_flags = get_headers_dep_files_flags_factory(tracking_mode)
                if mk_dep_files_flags:
                    headers_dep_files = _HeadersDepFiles(
                        processor = cmd_args(compiler_info.dep_files_processor),
                        mk_flags = mk_dep_files_flags,
                        tag = headers_tag,
                        dep_tracking_mode = tracking_mode,
                    )

            argsfile_by_ext[ext.value] = _mk_argsfile(ctx, compiler_info, pre, ext, headers_tag, None)
            if absolute_path_prefix:
                abs_argsfile_by_ext[ext.value] = _mk_argsfile(ctx, compiler_info, pre, ext, abs_headers_tag, absolute_path_prefix)

            cxx_compile_cmd_by_ext[ext] = _CxxCompileCommand(
                base_compile_cmd = base_compile_cmd,
                argsfile = argsfile_by_ext[ext.value],
                headers_dep_files = headers_dep_files,
                compiler_type = compiler_info.compiler_type,
            )

        cxx_compile_cmd = cxx_compile_cmd_by_ext[ext]

        src_args = []
        src_args.extend(src.flags)
        src_args.extend(["-c", src.file])

        src_compile_command = CxxSrcCompileCommand(src = src.file, cxx_compile_cmd = cxx_compile_cmd, args = src_args, index = src.index)
        src_compile_cmds.append(src_compile_command)

    argsfile_by_ext.update(impl_params.additional.argsfiles.relative)
    abs_argsfile_by_ext.update(impl_params.additional.argsfiles.absolute)

    if header_only:
        return CxxCompileCommandOutput(comp_db_compile_cmds = src_compile_cmds)
    else:
        return CxxCompileCommandOutput(
            src_compile_cmds = src_compile_cmds,
            argsfiles = CompileArgsfiles(
                relative = argsfile_by_ext,
                absolute = abs_argsfile_by_ext,
            ),
            comp_db_compile_cmds = src_compile_cmds,
        )

def compile_cxx(
        ctx: AnalysisContext,
        src_compile_cmds: list[CxxSrcCompileCommand.type],
        pic: bool = False) -> list[CxxCompileOutput.type]:
    """
    For a given list of src_compile_cmds, generate output artifacts.
    """
    toolchain = get_cxx_toolchain_info(ctx)
    linker_info = toolchain.linker_info

    object_format = toolchain.object_format or CxxObjectFormat("native")
    bitcode_args = cmd_args()
    if linker_info.lto_mode == LtoMode("none"):
        if toolchain.object_format == CxxObjectFormat("bitcode"):
            bitcode_args.add("-emit-llvm")
            object_format = CxxObjectFormat("bitcode")
        elif toolchain.object_format == CxxObjectFormat("embedded-bitcode"):
            bitcode_args.add("-fembed-bitcode")
            object_format = CxxObjectFormat("embedded-bitcode")
    else:
        object_format = CxxObjectFormat("bitcode")

    objects = []
    for src_compile_cmd in src_compile_cmds:
        identifier = src_compile_cmd.src.short_path
        if src_compile_cmd.index != None:
            # Add a unique postfix if we have duplicate source files with different flags
            identifier = identifier + "_" + str(src_compile_cmd.index)

        filename_base = identifier + (".pic" if pic else "")
        object = ctx.actions.declare_output(
            "__objects__",
            "{}.{}".format(filename_base, linker_info.object_file_extension),
        )

        cmd = cmd_args(src_compile_cmd.cxx_compile_cmd.base_compile_cmd)

        compiler_type = src_compile_cmd.cxx_compile_cmd.compiler_type
        cmd.add(get_output_flags(compiler_type, object))

        args = cmd_args()

        if pic:
            args.add(get_pic_flags(compiler_type))

        args.add(src_compile_cmd.cxx_compile_cmd.argsfile.cmd_form)
        args.add(src_compile_cmd.args)

        cmd.add(args)
        cmd.add(bitcode_args)

        action_dep_files = {}

        headers_dep_files = src_compile_cmd.cxx_compile_cmd.headers_dep_files
        if headers_dep_files:
            dep_file = ctx.actions.declare_output(
                paths.join("__dep_files__", filename_base),
            ).as_output()

            processor_flags, compiler_flags = headers_dep_files.mk_flags(ctx.actions, filename_base, src_compile_cmd.src)
            cmd.add(compiler_flags)

            # API: First argument is the dep file source path, second is the
            # dep file destination path, other arguments are the actual compile
            # command.
            cmd = cmd_args([
                headers_dep_files.processor,
                headers_dep_files.dep_tracking_mode.value,
                processor_flags,
                headers_dep_files.tag.tag_artifacts(dep_file),
                cmd,
            ])

            action_dep_files["headers"] = headers_dep_files.tag

        if pic:
            identifier += " (pic)"

        clang_remarks = None
        if toolchain.clang_remarks and compiler_type == "clang":
            args.add(["-fsave-optimization-record", "-fdiagnostics-show-hotness", "-foptimization-record-passes=" + toolchain.clang_remarks])
            clang_remarks = ctx.actions.declare_output(
                paths.join("__objects__", "{}.opt.yaml".format(filename_base)),
            )
            cmd.hidden(clang_remarks.as_output())

        clang_trace = None
        if toolchain.clang_trace and compiler_type == "clang":
            args.add(["-ftime-trace"])
            clang_trace = ctx.actions.declare_output(
                paths.join("__objects__", "{}.json".format(filename_base)),
            )
            cmd.hidden(clang_trace.as_output())

        ctx.actions.run(cmd, category = "cxx_compile", identifier = identifier, dep_files = action_dep_files)

        # If we're building with split debugging, where the debug info is in the
        # original object, then add the object as external debug info, *unless*
        # we're doing LTO, which generates debug info at link time (*except* for
        # fat LTO, which still generates native code and, therefore, debug info).
        object_has_external_debug_info = (
            toolchain.split_debug_mode == SplitDebugMode("single") and
            linker_info.lto_mode in (LtoMode("none"), LtoMode("fat"))
        )

        objects.append(CxxCompileOutput(
            object = object,
            object_format = object_format,
            object_has_external_debug_info = object_has_external_debug_info,
            clang_remarks = clang_remarks,
            clang_trace = clang_trace,
        ))

    return objects

def _validate_target_headers(ctx: AnalysisContext, preprocessor: list[CPreprocessor.type]):
    path_to_artifact = {}
    all_headers = flatten([x.headers for x in preprocessor])
    for header in all_headers:
        header_path = paths.join(header.namespace, header.name)
        artifact = path_to_artifact.get(header_path)
        if artifact != None:
            if artifact != header.artifact:
                fail("Conflicting headers {} and {} map to {} in target {}".format(artifact, header.artifact, header_path, ctx.label))
        else:
            path_to_artifact[header_path] = header.artifact

def _get_compiler_info(toolchain: "CxxToolchainInfo", ext: CxxExtension.type) -> "_compiler_info":
    compiler_info = None
    if ext.value in (".cpp", ".cc", ".mm", ".cxx", ".c++", ".h", ".hpp"):
        compiler_info = toolchain.cxx_compiler_info
    elif ext.value in (".c", ".m"):
        compiler_info = toolchain.c_compiler_info
    elif ext.value in (".s", ".S"):
        compiler_info = toolchain.as_compiler_info
    elif ext.value == ".cu":
        compiler_info = toolchain.cuda_compiler_info
    elif ext.value == ".hip":
        compiler_info = toolchain.hip_compiler_info
    elif ext.value in (".asm", ".asmpp"):
        compiler_info = toolchain.asm_compiler_info
    else:
        # This should be unreachable as long as we handle all enum values
        fail("Unknown C++ extension: " + ext.value)

    if not compiler_info:
        fail("Could not find compiler for extension `{ext}`".format(ext = ext.value))

    return compiler_info

def _get_compile_base(compiler_info: "_compiler_info") -> cmd_args:
    """
    Given a compiler info returned by _get_compiler_info, form the base compile args.
    """

    cmd = cmd_args(compiler_info.compiler)

    return cmd

def _dep_file_type(ext: CxxExtension.type) -> [DepFileType.type, None]:
    # Raw assembly doesn't make sense to capture dep files for.
    if ext.value in (".s", ".S", ".asm"):
        return None
    elif ext.value == ".hip":
        # TODO (T118797886): HipCompilerInfo doesn't have dep files processor.
        # Should it?
        return None

    # Return the file type aswell
    if ext.value in (".cpp", ".cc", ".mm", ".cxx", ".c++", ".h", ".hpp"):
        return DepFileType("cpp")
    elif ext.value in (".c", ".m"):
        return DepFileType("c")
    elif ext.value == ".cu":
        return DepFileType("cuda")
    elif ext.value in (".asmpp"):
        return DepFileType("asm")
    else:
        # This should be unreachable as long as we handle all enum values
        fail("Unknown C++ extension: " + ext.value)

def _add_compiler_info_flags(compiler_info: "_compiler_info", ext: CxxExtension.type, cmd: cmd_args):
    cmd.add(compiler_info.preprocessor_flags or [])
    cmd.add(compiler_info.compiler_flags or [])
    cmd.add(get_flags_for_reproducible_build(compiler_info.compiler_type))

    if ext.value not in (".asm", ".asmpp"):
        # Clang's asm compiler doesn't support colorful output, so we skip this there.
        cmd.add(get_flags_for_colorful_output(compiler_info.compiler_type))

def _mk_argsfile(
        ctx: AnalysisContext,
        compiler_info: "_compiler_info",
        preprocessor: CPreprocessorInfo.type,
        ext: CxxExtension.type,
        headers_tag: "artifact_tag",
        absolute_path_prefix: [str, None]) -> CompileArgsfile.type:
    """
    Generate and return an {ext}.argsfile artifact and command args that utilize the argsfile.
    """
    args = cmd_args()

    _add_compiler_info_flags(compiler_info, ext, args)

    if absolute_path_prefix:
        args.add(preprocessor.set.project_as_args("abs_args"))
    else:
        args.add(headers_tag.tag_artifacts(preprocessor.set.project_as_args("args")))

    # Different preprocessors will contain whether to use modules,
    # and the modulemap to use, so we need to get the final outcome.
    if preprocessor.set.reduce("uses_modules"):
        args.add(headers_tag.tag_artifacts(preprocessor.set.project_as_args("modular_args")))

    args.add(cxx_attr_preprocessor_flags(ctx, ext.value))
    args.add(get_flags_for_compiler_type(compiler_info.compiler_type))
    args.add(_attr_compiler_flags(ctx, ext.value))
    args.add(headers_tag.tag_artifacts(preprocessor.set.project_as_args("include_dirs")))

    # Workaround as that's not precompiled, but working just as prefix header.
    # Another thing is that it's clang specific, should be generalized.
    if ctx.attrs.precompiled_header != None:
        args.add(["-include", headers_tag.tag_artifacts(ctx.attrs.precompiled_header[CPrecompiledHeaderInfo].header)])
    if ctx.attrs.prefix_header != None:
        args.add(["-include", headers_tag.tag_artifacts(ctx.attrs.prefix_header)])

    # To convert relative paths to absolute, we utilize/expect the `./` marker to symbolize relative paths.
    if absolute_path_prefix:
        args.replace_regex("\\./", absolute_path_prefix + "/")

    # Create a copy of the args so that we can continue to modify it later.
    args_without_file_prefix_args = cmd_args(args)

    # Put file_prefix_args in argsfile directly, make sure they do not appear when evaluating $(cxxppflags)
    # to avoid "argument too long" errors
    if absolute_path_prefix:
        args.add(cmd_args(preprocessor.set.project_as_args("abs_file_prefix_args")))
    else:
        args.add(cmd_args(preprocessor.set.project_as_args("file_prefix_args")))

    shell_quoted_args = cmd_args(args, quote = "shell")

    file_name = ext.value + ("-abs.argsfile" if absolute_path_prefix else ".argsfile")
    argsfile, _ = ctx.actions.write(file_name, shell_quoted_args, allow_args = True)

    input_args = [args]

    cmd_form = cmd_args(argsfile, format = "@{}").hidden(input_args)

    return CompileArgsfile(
        file = argsfile,
        cmd_form = cmd_form,
        input_args = input_args,
        args = shell_quoted_args,
        args_without_file_prefix_args = args_without_file_prefix_args,
    )

def _attr_compiler_flags(ctx: AnalysisContext, ext: str) -> list[""]:
    return (
        cxx_by_language_ext(ctx.attrs.lang_compiler_flags, ext) +
        flatten(cxx_by_platform(ctx, ctx.attrs.platform_compiler_flags)) +
        flatten(cxx_by_platform(ctx, cxx_by_language_ext(ctx.attrs.lang_platform_compiler_flags, ext))) +
        # ctx.attrs.compiler_flags need to come last to preserve buck1 ordering, this prevents compiler
        # flags ordering-dependent build errors
        ctx.attrs.compiler_flags
    )

def _get_dep_tracking_mode(toolchain: "provider", file_type: DepFileType.type) -> DepTrackingMode.type:
    if file_type == DepFileType("cpp") or file_type == DepFileType("c"):
        return toolchain.cpp_dep_tracking_mode
    elif file_type == DepFileType("cuda"):
        return toolchain.cuda_dep_tracking_mode
    else:
        return DepTrackingMode("makefile")
