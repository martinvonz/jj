# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//cxx:cxx_toolchain_types.bzl", "AsCompilerInfo", "AsmCompilerInfo", "BinaryUtilitiesInfo", "CCompilerInfo", "CudaCompilerInfo", "CxxCompilerInfo", "CxxObjectFormat", "DepTrackingMode", "DistLtoToolsInfo", "HipCompilerInfo", "LinkerInfo", "PicBehavior", "StripFlagsInfo", "cxx_toolchain_infos")
load("@prelude//cxx:debug.bzl", "SplitDebugMode")
load("@prelude//cxx:headers.bzl", "HeaderMode", "HeadersAsRawHeadersMode")
load("@prelude//cxx:linker.bzl", "LINKERS", "is_pdb_generated")
load("@prelude//linking:link_info.bzl", "LinkOrdering", "LinkStyle")
load("@prelude//linking:lto.bzl", "LtoMode", "lto_compiler_flags")
load("@prelude//utils:utils.bzl", "value_or")
load("@prelude//decls/cxx_rules.bzl", "cxx_rules")

def cxx_toolchain_impl(ctx):
    c_compiler = _get_maybe_wrapped_msvc(ctx.attrs.c_compiler[RunInfo], ctx.attrs.c_compiler_type or ctx.attrs.compiler_type, ctx.attrs._msvc_hermetic_exec[RunInfo])

    lto_mode = LtoMode(ctx.attrs.lto_mode)
    if lto_mode != LtoMode("none"):
        object_format = "bitcode"
    else:
        object_format = ctx.attrs.object_format if ctx.attrs.object_format else "native"

    c_lto_flags = lto_compiler_flags(lto_mode)

    c_info = CCompilerInfo(
        compiler = c_compiler,
        compiler_type = ctx.attrs.c_compiler_type or ctx.attrs.compiler_type,
        compiler_flags = cmd_args(ctx.attrs.c_compiler_flags).add(c_lto_flags),
        preprocessor = c_compiler,
        preprocessor_flags = cmd_args(ctx.attrs.c_preprocessor_flags),
        dep_files_processor = ctx.attrs._dep_files_processor[RunInfo],
    )
    cxx_compiler = _get_maybe_wrapped_msvc(ctx.attrs.cxx_compiler[RunInfo], ctx.attrs.cxx_compiler_type or ctx.attrs.compiler_type, ctx.attrs._msvc_hermetic_exec[RunInfo])
    cxx_info = CxxCompilerInfo(
        compiler = cxx_compiler,
        compiler_type = ctx.attrs.cxx_compiler_type or ctx.attrs.compiler_type,
        compiler_flags = cmd_args(ctx.attrs.cxx_compiler_flags).add(c_lto_flags),
        preprocessor = cxx_compiler,
        preprocessor_flags = cmd_args(ctx.attrs.cxx_preprocessor_flags),
        dep_files_processor = ctx.attrs._dep_files_processor[RunInfo],
    )
    asm_info = AsmCompilerInfo(
        compiler = ctx.attrs.asm_compiler[RunInfo],
        compiler_type = ctx.attrs.asm_compiler_type or ctx.attrs.compiler_type,
        compiler_flags = cmd_args(ctx.attrs.asm_compiler_flags),
        preprocessor_flags = cmd_args(ctx.attrs.asm_preprocessor_flags),
        dep_files_processor = ctx.attrs._dep_files_processor[RunInfo],
    ) if ctx.attrs.asm_compiler else None
    as_info = AsCompilerInfo(
        compiler = ctx.attrs.assembler[RunInfo],
        compiler_type = ctx.attrs.assembler_type or ctx.attrs.compiler_type,
        compiler_flags = cmd_args(ctx.attrs.assembler_flags),
        preprocessor_flags = cmd_args(ctx.attrs.assembler_preprocessor_flags),
        dep_files_processor = ctx.attrs._dep_files_processor[RunInfo],
    ) if ctx.attrs.assembler else None
    cuda_info = CudaCompilerInfo(
        compiler = ctx.attrs.cuda_compiler[RunInfo],
        compiler_type = ctx.attrs.cuda_compiler_type or ctx.attrs.compiler_type,
        compiler_flags = cmd_args(ctx.attrs.cuda_compiler_flags),
        preprocessor_flags = cmd_args(ctx.attrs.cuda_preprocessor_flags),
        dep_files_processor = ctx.attrs._dep_files_processor[RunInfo],
    ) if ctx.attrs.cuda_compiler else None
    hip_info = HipCompilerInfo(
        compiler = ctx.attrs.hip_compiler[RunInfo],
        compiler_type = ctx.attrs.hip_compiler_type or ctx.attrs.compiler_type,
        compiler_flags = cmd_args(ctx.attrs.hip_compiler_flags),
        preprocessor_flags = cmd_args(ctx.attrs.hip_preprocessor_flags),
    ) if ctx.attrs.hip_compiler else None

    linker_info = LinkerInfo(
        archiver = ctx.attrs.archiver[RunInfo],
        archiver_flags = cmd_args(ctx.attrs.archiver_flags),
        archiver_supports_argfiles = ctx.attrs.archiver_supports_argfiles,
        archiver_type = ctx.attrs.archiver_type,
        archive_contents = ctx.attrs.archive_contents,
        archive_objects_locally = False,
        binary_extension = value_or(ctx.attrs.binary_extension, ""),
        generate_linker_maps = ctx.attrs.generate_linker_maps,
        is_pdb_generated = is_pdb_generated(ctx.attrs.linker_type, ctx.attrs.linker_flags),
        link_binaries_locally = not value_or(ctx.attrs.cache_links, True),
        link_libraries_locally = False,
        link_style = LinkStyle("static"),
        link_weight = 1,
        link_ordering = ctx.attrs.link_ordering,
        linker = ctx.attrs.linker[RunInfo],
        linker_flags = cmd_args(ctx.attrs.linker_flags).add(c_lto_flags),
        lto_mode = lto_mode,
        object_file_extension = ctx.attrs.object_file_extension or "o",
        shlib_interfaces = "disabled",
        independent_shlib_interface_linker_flags = ctx.attrs.shared_library_interface_flags,
        requires_archives = value_or(ctx.attrs.requires_archives, True),
        requires_objects = value_or(ctx.attrs.requires_objects, False),
        supports_distributed_thinlto = ctx.attrs.supports_distributed_thinlto,
        shared_dep_runtime_ld_flags = ctx.attrs.shared_dep_runtime_ld_flags,
        shared_library_name_format = _get_shared_library_name_format(ctx),
        shared_library_versioned_name_format = _get_shared_library_versioned_name_format(ctx),
        static_dep_runtime_ld_flags = ctx.attrs.static_dep_runtime_ld_flags,
        static_library_extension = ctx.attrs.static_library_extension or "a",
        static_pic_dep_runtime_ld_flags = ctx.attrs.static_pic_dep_runtime_ld_flags,
        type = ctx.attrs.linker_type,
        use_archiver_flags = ctx.attrs.use_archiver_flags,
    )

    utilities_info = BinaryUtilitiesInfo(
        nm = ctx.attrs.nm[RunInfo],
        objcopy = ctx.attrs.objcopy_for_shared_library_interface[RunInfo],
        ranlib = ctx.attrs.ranlib[RunInfo] if ctx.attrs.ranlib else None,
        strip = ctx.attrs.strip[RunInfo],
        dwp = None,
        bolt_msdk = None,
    )

    strip_flags_info = StripFlagsInfo(
        strip_debug_flags = ctx.attrs.strip_debug_flags,
        strip_non_global_flags = ctx.attrs.strip_non_global_flags,
        strip_all_flags = ctx.attrs.strip_all_flags,
    )

    platform_name = ctx.attrs.platform_name or ctx.attrs.name
    return [
        DefaultInfo(),
    ] + cxx_toolchain_infos(
        platform_name = platform_name,
        linker_info = linker_info,
        binary_utilities_info = utilities_info,
        bolt_enabled = value_or(ctx.attrs.bolt_enabled, False),
        c_compiler_info = c_info,
        cxx_compiler_info = cxx_info,
        asm_compiler_info = asm_info,
        as_compiler_info = as_info,
        cuda_compiler_info = cuda_info,
        hip_compiler_info = hip_info,
        header_mode = _get_header_mode(ctx),
        llvm_link = ctx.attrs.llvm_link[RunInfo] if ctx.attrs.llvm_link else None,
        object_format = CxxObjectFormat(object_format),
        headers_as_raw_headers_mode = HeadersAsRawHeadersMode(ctx.attrs.headers_as_raw_headers_mode) if ctx.attrs.headers_as_raw_headers_mode != None else None,
        conflicting_header_basename_allowlist = ctx.attrs.conflicting_header_basename_exemptions,
        mk_hmap = ctx.attrs._mk_hmap[RunInfo],
        mk_comp_db = ctx.attrs._mk_comp_db,
        pic_behavior = PicBehavior(ctx.attrs.pic_behavior),
        split_debug_mode = SplitDebugMode(ctx.attrs.split_debug_mode),
        strip_flags_info = strip_flags_info,
        # TODO(T138705365): Turn on dep files by default
        use_dep_files = value_or(ctx.attrs.use_dep_files, _get_default_use_dep_files(platform_name)),
        clang_remarks = ctx.attrs.clang_remarks,
        clang_trace = value_or(ctx.attrs.clang_trace, False),
        cpp_dep_tracking_mode = DepTrackingMode(ctx.attrs.cpp_dep_tracking_mode),
        cuda_dep_tracking_mode = DepTrackingMode(ctx.attrs.cuda_dep_tracking_mode),
    )

def cxx_toolchain_extra_attributes(is_toolchain_rule):
    dep_type = attrs.exec_dep if is_toolchain_rule else attrs.dep
    return {
        "archiver": dep_type(providers = [RunInfo]),
        "archiver_supports_argfiles": attrs.bool(default = False),
        "asm_compiler": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "asm_preprocessor": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "assembler": dep_type(providers = [RunInfo]),
        "assembler_preprocessor": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "bolt_enabled": attrs.bool(default = False),
        "c_compiler": dep_type(providers = [RunInfo]),
        "clang_remarks": attrs.option(attrs.string(), default = None),
        "clang_trace": attrs.option(attrs.bool(), default = None),
        "cpp_dep_tracking_mode": attrs.enum(DepTrackingMode.values(), default = "makefile"),
        "cuda_compiler": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "cuda_dep_tracking_mode": attrs.enum(DepTrackingMode.values(), default = "makefile"),
        "cxx_compiler": dep_type(providers = [RunInfo]),
        "generate_linker_maps": attrs.bool(default = False),
        "hip_compiler": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "link_ordering": attrs.enum(LinkOrdering.values(), default = "preorder"),
        "linker": dep_type(providers = [RunInfo]),
        "llvm_link": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "lto_mode": attrs.enum(LtoMode.values(), default = "none"),
        "nm": dep_type(providers = [RunInfo]),
        "objcopy_for_shared_library_interface": dep_type(providers = [RunInfo]),
        "object_format": attrs.enum(CxxObjectFormat.values(), default = "native"),
        "pic_behavior": attrs.enum(PicBehavior.values(), default = "supported"),
        # A placeholder tool that can be used to set up toolchain constraints.
        # Useful when fat and thin toolchahins share the same underlying tools via `command_alias()`,
        # which requires setting up separate platform-specific aliases with the correct constraints.
        "placeholder_tool": attrs.option(dep_type(providers = [RunInfo]), default = None),
        # Used for resolving any 'platform_*' attributes.
        "platform_name": attrs.option(attrs.string(), default = None),
        "private_headers_symlinks_enabled": attrs.bool(default = True),
        "public_headers_symlinks_enabled": attrs.bool(default = True),
        "ranlib": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "requires_objects": attrs.bool(default = False),
        "split_debug_mode": attrs.enum(SplitDebugMode.values(), default = "none"),
        "strip": dep_type(providers = [RunInfo]),
        "supports_distributed_thinlto": attrs.bool(default = False),
        "use_archiver_flags": attrs.bool(default = True),
        "use_dep_files": attrs.option(attrs.bool(), default = None),
        "_dep_files_processor": dep_type(providers = [RunInfo], default = "prelude//cxx/tools:dep_file_processor"),
        "_dist_lto_tools": attrs.default_only(dep_type(providers = [DistLtoToolsInfo], default = "prelude//cxx/dist_lto/tools:dist_lto_tools")),
        "_mk_comp_db": attrs.default_only(dep_type(providers = [RunInfo], default = "prelude//cxx/tools:make_comp_db")),
        # FIXME: prelude// should be standalone (not refer to fbsource//)
        "_mk_hmap": attrs.default_only(dep_type(providers = [RunInfo], default = "fbsource//xplat/buck2/tools/cxx:hmap_wrapper")),
        "_msvc_hermetic_exec": attrs.default_only(dep_type(providers = [RunInfo], default = "prelude//windows/tools:msvc_hermetic_exec")),
    }

def _cxx_toolchain_inheriting_target_platform_attrs():
    attrs = dict(cxx_rules.cxx_toolchain.attrs)
    attrs.update(cxx_toolchain_extra_attributes(is_toolchain_rule = True))
    return attrs

cxx_toolchain_inheriting_target_platform = rule(
    impl = cxx_toolchain_impl,
    attrs = _cxx_toolchain_inheriting_target_platform_attrs(),
    is_toolchain_rule = True,
)

_APPLE_PLATFORM_NAME_PREFIXES = [
    "iphonesimulator",
    "iphoneos",
    "maccatalyst",
    "macosx",
    "watchos",
    "watchsimulator",
    "appletvos",
    "appletvsimulator",
]

def _get_default_use_dep_files(platform_name: str) -> bool:
    # All Apple platforms use Clang which supports the standard dep files format
    for apple_platform_name_prefix in _APPLE_PLATFORM_NAME_PREFIXES:
        if apple_platform_name_prefix in platform_name:
            return True
    return False

def _get_header_mode(ctx: AnalysisContext) -> HeaderMode.type:
    if ctx.attrs.use_header_map:
        if ctx.attrs.private_headers_symlinks_enabled or ctx.attrs.public_headers_symlinks_enabled:
            return HeaderMode("symlink_tree_with_header_map")
        else:
            return HeaderMode("header_map_only")
    else:
        return HeaderMode("symlink_tree_only")

def _get_shared_library_name_format(ctx: AnalysisContext) -> str:
    linker_type = ctx.attrs.linker_type
    extension = ctx.attrs.shared_library_extension
    if extension == "":
        extension = LINKERS[linker_type].default_shared_library_extension
    prefix = "" if extension == "dll" else "lib"
    return prefix + "{}." + extension

def _get_shared_library_versioned_name_format(ctx: AnalysisContext) -> str:
    linker_type = ctx.attrs.linker_type
    extension_format = ctx.attrs.shared_library_versioned_extension_format.replace("%s", "{}")
    if extension_format == "":
        extension_format = LINKERS[linker_type].default_shared_library_versioned_extension_format
    prefix = "" if extension_format == "dll" else "lib"
    return prefix + "{}." + extension_format

def _get_maybe_wrapped_msvc(compiler: "RunInfo", compiler_type: str, msvc_hermetic_exec: "RunInfo") -> "RunInfo":
    if compiler_type == "windows":
        return RunInfo(args = cmd_args(msvc_hermetic_exec, compiler))
    return compiler
