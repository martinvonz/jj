# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//cxx:cxx_toolchain_types.bzl", "AsCompilerInfo", "AsmCompilerInfo", "BinaryUtilitiesInfo", "CCompilerInfo", "CxxCompilerInfo", "CxxObjectFormat", "CxxPlatformInfo", "CxxToolchainInfo", "LinkerInfo", "LinkerType", "PicBehavior", "StripFlagsInfo", "cxx_toolchain_infos")
load("@prelude//cxx:debug.bzl", "SplitDebugMode")
load("@prelude//cxx:headers.bzl", "HeaderMode")
load("@prelude//cxx:linker.bzl", "is_pdb_generated")
load(
    "@prelude//linking:link_info.bzl",
    "LinkStyle",
)
load("@prelude//linking:lto.bzl", "LtoMode")
load("@prelude//user:rule_spec.bzl", "RuleRegistrationSpec")
load("@prelude//utils:pick.bzl", _pick = "pick", _pick_and_add = "pick_and_add", _pick_bin = "pick_bin", _pick_dep = "pick_dep")
load("@prelude//utils:utils.bzl", "value_or")

def _cxx_toolchain_override(ctx):
    base_toolchain = ctx.attrs.base[CxxToolchainInfo]
    base_as_info = base_toolchain.as_compiler_info
    as_info = AsCompilerInfo(
        compiler = _pick_bin(ctx.attrs.as_compiler, base_as_info.compiler),
        compiler_type = base_as_info.compiler_type,
        compiler_flags = _pick(ctx.attrs.as_compiler_flags, base_as_info.compiler_flags),
        preprocessor = _pick_bin(ctx.attrs.as_compiler, base_as_info.preprocessor),
        preprocessor_type = base_as_info.preprocessor_type,
        preprocessor_flags = _pick(ctx.attrs.as_preprocessor_flags, base_as_info.preprocessor_flags),
        dep_files_processor = base_as_info.dep_files_processor,
    )
    asm_info = base_toolchain.asm_compiler_info
    if asm_info != None:
        asm_info = AsmCompilerInfo(
            compiler = _pick_bin(ctx.attrs.asm_compiler, asm_info.compiler),
            compiler_type = asm_info.compiler_type,
            compiler_flags = _pick(ctx.attrs.asm_compiler_flags, asm_info.compiler_flags),
            preprocessor = _pick_bin(ctx.attrs.asm_compiler, asm_info.preprocessor),
            preprocessor_type = asm_info.preprocessor_type,
            preprocessor_flags = _pick(ctx.attrs.asm_preprocessor_flags, asm_info.preprocessor_flags),
            dep_files_processor = asm_info.dep_files_processor,
        )
    base_c_info = base_toolchain.c_compiler_info
    c_info = CCompilerInfo(
        compiler = _pick_bin(ctx.attrs.c_compiler, base_c_info.compiler),
        compiler_type = base_c_info.compiler_type,
        compiler_flags = _pick_and_add(ctx.attrs.c_compiler_flags, ctx.attrs.additional_c_compiler_flags, base_c_info.compiler_flags),
        preprocessor = _pick_bin(ctx.attrs.c_compiler, base_c_info.preprocessor),
        preprocessor_type = base_c_info.preprocessor_type,
        preprocessor_flags = _pick(ctx.attrs.c_preprocessor_flags, base_c_info.preprocessor_flags),
        dep_files_processor = base_c_info.dep_files_processor,
    )
    base_cxx_info = base_toolchain.cxx_compiler_info
    cxx_info = CxxCompilerInfo(
        compiler = _pick_bin(ctx.attrs.cxx_compiler, base_cxx_info.compiler),
        compiler_type = base_cxx_info.compiler_type,
        compiler_flags = _pick_and_add(ctx.attrs.cxx_compiler_flags, ctx.attrs.additional_cxx_compiler_flags, base_cxx_info.compiler_flags),
        preprocessor = _pick_bin(ctx.attrs.cxx_compiler, base_cxx_info.preprocessor),
        preprocessor_type = base_cxx_info.preprocessor_type,
        preprocessor_flags = _pick(ctx.attrs.cxx_preprocessor_flags, base_cxx_info.preprocessor_flags),
        dep_files_processor = base_cxx_info.dep_files_processor,
    )
    base_linker_info = base_toolchain.linker_info
    linker_type = ctx.attrs.linker_type if ctx.attrs.linker_type != None else base_linker_info.type
    pdb_expected = is_pdb_generated(linker_type, ctx.attrs.linker_flags) if ctx.attrs.linker_flags != None else base_linker_info.is_pdb_generated

    # This handles case when linker type is overriden to non-windows from
    # windows but linker flags are inherited.
    # When it's changed from non-windows to windows but flags are not changed,
    # we can't inspect base linker flags and disable PDB subtargets.
    # This shouldn't be a problem because to use windows linker after non-windows
    # linker flags should be changed as well.
    pdb_expected = linker_type == "windows" and pdb_expected
    linker_info = LinkerInfo(
        archiver = _pick_bin(ctx.attrs.archiver, base_linker_info.archiver),
        archiver_type = base_linker_info.archiver_type,
        archiver_supports_argfiles = value_or(ctx.attrs.archiver_supports_argfiles, base_linker_info.archiver_supports_argfiles),
        archive_contents = base_linker_info.archive_contents,
        archive_objects_locally = value_or(ctx.attrs.archive_objects_locally, base_linker_info.archive_objects_locally),
        binary_extension = base_linker_info.binary_extension,
        generate_linker_maps = value_or(ctx.attrs.generate_linker_maps, base_linker_info.generate_linker_maps),
        link_binaries_locally = value_or(ctx.attrs.link_binaries_locally, base_linker_info.link_binaries_locally),
        link_libraries_locally = value_or(ctx.attrs.link_libraries_locally, base_linker_info.link_libraries_locally),
        link_style = LinkStyle(ctx.attrs.link_style) if ctx.attrs.link_style != None else base_linker_info.link_style,
        link_weight = value_or(ctx.attrs.link_weight, base_linker_info.link_weight),
        link_ordering = base_linker_info.link_ordering,
        linker = _pick_bin(ctx.attrs.linker, base_linker_info.linker),
        linker_flags = _pick(ctx.attrs.linker_flags, base_linker_info.linker_flags),
        lto_mode = LtoMode(value_or(ctx.attrs.lto_mode, base_linker_info.lto_mode.value)),
        object_file_extension = base_linker_info.object_file_extension,
        shlib_interfaces = base_linker_info.shlib_interfaces,
        mk_shlib_intf = _pick_dep(ctx.attrs.mk_shlib_intf, base_linker_info.mk_shlib_intf),
        requires_archives = base_linker_info.requires_archives,
        requires_objects = base_linker_info.requires_objects,
        supports_distributed_thinlto = base_linker_info.supports_distributed_thinlto,
        independent_shlib_interface_linker_flags = base_linker_info.independent_shlib_interface_linker_flags,
        shared_dep_runtime_ld_flags = [],
        shared_library_name_format = ctx.attrs.shared_library_name_format if ctx.attrs.shared_library_name_format != None else base_linker_info.shared_library_name_format,
        shared_library_versioned_name_format = ctx.attrs.shared_library_versioned_name_format if ctx.attrs.shared_library_versioned_name_format != None else base_linker_info.shared_library_versioned_name_format,
        static_dep_runtime_ld_flags = [],
        static_pic_dep_runtime_ld_flags = [],
        static_library_extension = base_linker_info.static_library_extension,
        type = linker_type,
        use_archiver_flags = value_or(ctx.attrs.use_archiver_flags, base_linker_info.use_archiver_flags),
        force_full_hybrid_if_capable = value_or(ctx.attrs.force_full_hybrid_if_capable, base_linker_info.force_full_hybrid_if_capable),
        is_pdb_generated = pdb_expected,
    )

    base_binary_utilities_info = base_toolchain.binary_utilities_info
    binary_utilities_info = BinaryUtilitiesInfo(
        nm = _pick_bin(ctx.attrs.nm, base_binary_utilities_info.nm),
        objcopy = _pick_bin(ctx.attrs.objcopy, base_binary_utilities_info.objcopy),
        ranlib = _pick_bin(ctx.attrs.ranlib, base_binary_utilities_info.ranlib),
        strip = _pick_bin(ctx.attrs.strip, base_binary_utilities_info.strip),
        dwp = base_binary_utilities_info.dwp,
        bolt_msdk = base_binary_utilities_info.bolt_msdk,
    )

    base_strip_flags_info = base_toolchain.strip_flags_info
    strip_flags_info = StripFlagsInfo(
        strip_debug_flags = _pick(ctx.attrs.strip_debug_flags, base_strip_flags_info.strip_debug_flags),
        strip_non_global_flags = _pick(ctx.attrs.strip_non_global_flags, base_strip_flags_info.strip_non_global_flags),
        strip_all_flags = _pick(ctx.attrs.strip_all_flags, base_strip_flags_info.strip_all_flags),
    )

    return [
        DefaultInfo(),
    ] + cxx_toolchain_infos(
        platform_name = ctx.attrs.platform_name if ctx.attrs.platform_name != None else ctx.attrs.base[CxxPlatformInfo].name,
        platform_deps_aliases = ctx.attrs.platform_deps_aliases if ctx.attrs.platform_deps_aliases != None else [],
        linker_info = linker_info,
        as_compiler_info = as_info,
        asm_compiler_info = asm_info,
        binary_utilities_info = binary_utilities_info,
        bolt_enabled = value_or(ctx.attrs.bolt_enabled, base_toolchain.bolt_enabled),
        c_compiler_info = c_info,
        cxx_compiler_info = cxx_info,
        llvm_link = ctx.attrs.llvm_link if ctx.attrs.llvm_link != None else base_toolchain.llvm_link,
        # the rest are used without overrides
        cuda_compiler_info = base_toolchain.cuda_compiler_info,
        hip_compiler_info = base_toolchain.hip_compiler_info,
        header_mode = HeaderMode(ctx.attrs.header_mode) if ctx.attrs.header_mode != None else base_toolchain.header_mode,
        headers_as_raw_headers_mode = base_toolchain.headers_as_raw_headers_mode,
        mk_comp_db = _pick_bin(ctx.attrs.mk_comp_db, base_toolchain.mk_comp_db),
        mk_hmap = _pick_bin(ctx.attrs.mk_hmap, base_toolchain.mk_hmap),
        dist_lto_tools_info = base_toolchain.dist_lto_tools_info,
        use_dep_files = base_toolchain.use_dep_files,
        clang_remarks = base_toolchain.clang_remarks,
        clang_trace = base_toolchain.clang_trace,
        object_format = CxxObjectFormat(ctx.attrs.object_format) if ctx.attrs.object_format != None else base_toolchain.object_format,
        conflicting_header_basename_allowlist = base_toolchain.conflicting_header_basename_allowlist,
        strip_flags_info = strip_flags_info,
        pic_behavior = PicBehavior(ctx.attrs.pic_behavior) if ctx.attrs.pic_behavior != None else base_toolchain.pic_behavior.value,
        split_debug_mode = SplitDebugMode(value_or(ctx.attrs.split_debug_mode, base_toolchain.split_debug_mode.value)),
    )

def _cxx_toolchain_override_inheriting_target_platform_attrs(is_toolchain_rule):
    dep_type = attrs.exec_dep if is_toolchain_rule else attrs.dep
    base_dep_type = attrs.toolchain_dep if is_toolchain_rule else attrs.dep
    return {
        "additional_c_compiler_flags": attrs.option(attrs.list(attrs.arg()), default = None),
        "additional_cxx_compiler_flags": attrs.option(attrs.list(attrs.arg()), default = None),
        "archive_objects_locally": attrs.option(attrs.bool(), default = None),
        "archiver": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "archiver_supports_argfiles": attrs.option(attrs.bool(), default = None),
        "as_compiler": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "as_compiler_flags": attrs.option(attrs.list(attrs.arg()), default = None),
        "as_preprocessor_flags": attrs.option(attrs.list(attrs.arg()), default = None),
        "asm_compiler": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "asm_compiler_flags": attrs.option(attrs.list(attrs.arg()), default = None),
        "asm_preprocessor_flags": attrs.option(attrs.list(attrs.arg()), default = None),
        "base": base_dep_type(providers = [CxxToolchainInfo]),
        "bolt_enabled": attrs.option(attrs.bool(), default = None),
        "c_compiler": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "c_compiler_flags": attrs.option(attrs.list(attrs.arg()), default = None),
        "c_preprocessor_flags": attrs.option(attrs.list(attrs.arg()), default = None),
        "cxx_compiler": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "cxx_compiler_flags": attrs.option(attrs.list(attrs.arg()), default = None),
        "cxx_preprocessor_flags": attrs.option(attrs.list(attrs.arg()), default = None),
        "force_full_hybrid_if_capable": attrs.option(attrs.bool(), default = None),
        "generate_linker_maps": attrs.option(attrs.bool(), default = None),
        "header_mode": attrs.option(attrs.enum(HeaderMode.values()), default = None),
        "link_binaries_locally": attrs.option(attrs.bool(), default = None),
        "link_libraries_locally": attrs.option(attrs.bool(), default = None),
        "link_style": attrs.option(attrs.enum(LinkStyle.values()), default = None),
        "link_weight": attrs.option(attrs.int(), default = None),
        "linker": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "linker_flags": attrs.option(attrs.list(attrs.arg()), default = None),
        "linker_type": attrs.option(attrs.enum(LinkerType), default = None),
        "llvm_link": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "lto_mode": attrs.option(attrs.enum(LtoMode.values()), default = None),
        "mk_comp_db": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "mk_hmap": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "mk_shlib_intf": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "nm": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "objcopy": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "object_format": attrs.enum(CxxObjectFormat.values(), default = "native"),
        "pic_behavior": attrs.enum(PicBehavior.values(), default = "supported"),
        "platform_deps_aliases": attrs.option(attrs.list(attrs.string()), default = None),
        "platform_name": attrs.option(attrs.string(), default = None),
        "ranlib": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "shared_library_name_format": attrs.option(attrs.string(), default = None),
        "shared_library_versioned_name_format": attrs.option(attrs.string(), default = None),
        "split_debug_mode": attrs.option(attrs.enum(SplitDebugMode.values()), default = None),
        "strip": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "strip_all_flags": attrs.option(attrs.list(attrs.arg()), default = None),
        "strip_debug_flags": attrs.option(attrs.list(attrs.arg()), default = None),
        "strip_non_global_flags": attrs.option(attrs.list(attrs.arg()), default = None),
        "use_archiver_flags": attrs.option(attrs.bool(), default = None),
    }

cxx_toolchain_override_registration_spec = RuleRegistrationSpec(
    name = "cxx_toolchain_override",
    impl = _cxx_toolchain_override,
    attrs = _cxx_toolchain_override_inheriting_target_platform_attrs(is_toolchain_rule = False),
)

cxx_toolchain_override_inheriting_target_platform_registration_spec = RuleRegistrationSpec(
    name = "cxx_toolchain_override_inheriting_target_platform",
    impl = _cxx_toolchain_override,
    attrs = _cxx_toolchain_override_inheriting_target_platform_attrs(is_toolchain_rule = True),
    is_toolchain_rule = True,
)
