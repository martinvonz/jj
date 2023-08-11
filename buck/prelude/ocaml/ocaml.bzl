# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# Implementation of the OCaml build rules.

# IMPORTANT: Don't land your change without running these tests!
# ```
# buck2 build --num-threads 4 $(buck2 uquery "kind('ocaml_binary', 'fbcode//hphp/...')")
# ```
#
# If you are really, really keen, this command builds all hack, not just the
# OCaml binaries.
# ```
# buck2 build --num-threads 4 fbcode//hphp/hack/...
# ```

# To avoid name collisions (where '/' designates the build output
# directory root):
#

#   - Binaries (.opt) and libraries (.a, .cmxa) are written to '/'
#   - Where <mode> is one of 'bytecode', 'native' or 'expand':
#     - Generated sources are written to `/_<mode>_gen_`
#     - Intermedidate files (.cmi, .cmti, .cmt, .cmx, .o, .pp.ml, ...) are
#       written to `/_<mode>_obj_`
#
# For example given,
#   ocaml_binary(
#      name = "quux",
#      srcs = [
#          "quux/quux.ml",
#          "quux/corge/corge.ml"
#      ],
#   )
# the structure of the native build output will be (roughly)
# /
#   _native_obj_/
#     quux/quux.cmi
#     quux/quux.cmx
#     quux/quux.o
#     quux/corge/corge.cmi
#     quux/corge/corge.cmx
#     quux/corge/corge.o
#   quux.opt

load("@prelude//:local_only.bzl", "link_cxx_binary_locally")
load("@prelude//:paths.bzl", "paths")
load("@prelude//cxx:cxx_context.bzl", "get_cxx_platform_info", "get_cxx_toolchain_info")
load(
    "@prelude//cxx:cxx_link_utility.bzl",
    "make_link_args",
)
load(
    "@prelude//cxx:preprocessor.bzl",
    "CPreprocessorInfo",
    "cxx_merge_cpreprocessors",
)
load(
    "@prelude//linking:link_groups.bzl",
    "merge_link_group_lib_info",
)
load(
    "@prelude//linking:link_info.bzl",
    "LinkInfo",
    "LinkInfos",
    "LinkStyle",
    "MergedLinkInfo",
    "ObjectsLinkable",
    "create_merged_link_info",
    "get_link_args",
    "merge_link_infos",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "create_linkable_graph",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibraryInfo",
    "merge_shared_libraries",
)
load(
    "@prelude//python:python.bzl",
    "PythonLibraryInfo",
)
load("@prelude//utils:graph_utils.bzl", "breadth_first_traversal", "post_order_traversal")
load("@prelude//utils:platform_flavors_util.bzl", "by_platform")
load("@prelude//utils:utils.bzl", "filter_and_map_idx", "flatten")
load(":makefile.bzl", "parse_makefile")
load(":ocaml_toolchain_types.bzl", "OCamlLibraryInfo", "OCamlLinkInfo", "OCamlToolchainInfo", "OtherOutputsInfo", "merge_ocaml_link_infos", "merge_other_outputs_info")

BuildMode = enum("native", "bytecode", "expand")

# Native vs. bytecode compiler.
def _is_native(mode: "BuildMode") -> bool:
    return mode.value in ("native", "expand")

# The type of the return value of the `_compile()` function.
CompileResultInfo = record(
    # The .cmx file names in topological order
    cmxs_order = field("artifact"),
    # .o files (of .c files)
    stbs = field(["artifact"], []),
    # .o files (of .ml files)
    objs = field(["artifact"], []),
    # .cmi files
    cmis = field(["artifact"], []),
    # .cmo files
    cmos = field(["artifact"], []),
    # .cmx files
    cmxs = field(["artifact"], []),
    # .cmt files
    cmts = field(["artifact"], []),
    # .cmti files
    cmtis = field(["artifact"], []),
    # .pp.mli files
    ppmlis = field(["artifact"], []),
    # .pp.ml files
    ppmls = field(["artifact"], []),
)

def _compile_result_to_tuple(r):
    return (r.cmxs_order, r.stbs, r.objs, r.cmis, r.cmos, r.cmxs, r.cmts, r.cmtis, r.ppmlis, r.ppmls)

# ---

def _by_platform(ctx: AnalysisContext, xs: list[(str, list["_a"])]) -> list["_a"]:
    platform = get_cxx_platform_info(ctx).name
    return flatten(by_platform([platform], xs))

def _attr_deps(ctx: AnalysisContext) -> list[Dependency]:
    return ctx.attrs.deps + _by_platform(ctx, ctx.attrs.platform_deps)

def _attr_deps_merged_link_infos(ctx: AnalysisContext) -> list["MergedLinkInfo"]:
    return filter(None, [d.get(MergedLinkInfo) for d in _attr_deps(ctx)])

def _attr_deps_ocaml_link_infos(ctx: AnalysisContext) -> list["OCamlLinkInfo"]:
    return filter(None, [d.get(OCamlLinkInfo) for d in _attr_deps(ctx)])

def _attr_deps_other_outputs_infos(ctx: AnalysisContext) -> list["OtherOutputsInfo"]:
    return filter(None, [d.get(OtherOutputsInfo) for d in _attr_deps(ctx)])

# ---

# Rules

# We want to pass a series of arguments as a command, but the OCaml compiler
# only lets us pass a single script. Therefore, produce a script that contains
# many arguments.
def _mk_script(ctx: AnalysisContext, file: str, args: list[""], env: dict[str, ""]) -> cmd_args:
    lines = ["#!/usr/bin/env bash"]
    for name, val in env.items():
        lines.append(cmd_args(val, format = "export {}={{}}".format(name)))
    lines.append(cmd_args([cmd_args(args, quote = "shell"), "\"$@\""], delimiter = " "))
    script, _ = ctx.actions.write(
        file,
        lines,
        is_executable = True,
        allow_args = True,
    )
    return cmd_args(script).hidden(args, env.values())

# An environment in which a custom `bin` is at the head of `$PATH`.
def _mk_env(ctx: AnalysisContext) -> dict[str, cmd_args]:
    ocaml_toolchain = ctx.attrs._ocaml_toolchain[OCamlToolchainInfo]

    # "Partial linking" (via `ocamlopt.opt -output-obj`) emits calls to `ld -r
    # -o`. This is the `ld` that will be invoked. See [Note: What is
    # `binutils_ld`?] in `providers.bzl`.
    binutils_ld = ocaml_toolchain.binutils_ld
    binutils_as = ocaml_toolchain.binutils_as

    links = {}
    if binutils_as != None:
        links["as"] = binutils_as
    if binutils_ld != None:
        links["ld"] = binutils_ld

    if links:
        # A local `bin` dir of soft links.
        bin = ctx.actions.symlinked_dir("bin", links)

        # An environment in which `bin` is at the head of `$PATH`.
        return {"PATH": cmd_args(bin, format = "{}:\"$PATH\"")}
    else:
        return {}

# Pass '-cc cc.sh' to ocamlopt to use 'cc.sh' as the C compiler.
def _mk_cc(ctx: AnalysisContext, cc_args: list[""], cc_sh_filename: "") -> cmd_args:
    cxx_toolchain = get_cxx_toolchain_info(ctx)
    compiler = cxx_toolchain.c_compiler_info.compiler
    return _mk_script(ctx, cc_sh_filename, [compiler] + cc_args, {})

# Pass '-cc ld.sh' to ocamlopt to use 'ld.sh' as the C linker.
def _mk_ld(ctx: AnalysisContext, link_args: list[""], ld_sh_filename: "") -> cmd_args:
    cxx_toolchain = get_cxx_toolchain_info(ctx)
    linker = cxx_toolchain.linker_info.linker
    linker_flags = cxx_toolchain.linker_info.linker_flags
    return _mk_script(ctx, ld_sh_filename, [linker, linker_flags] + link_args, {})

# This should get called only once for any invocation of `ocaml_library_impl`,
# `ocaml_binary_impl` (or `prebuilt_ocaml_library_impl`) and choice of
# `build_mode`. It produces a script that forwards arguments to the ocaml
# compiler (one of `ocamlopt.opt` vs `ocamlc.opt` consistent with the value of
# `build_mode`) in the environment of a local 'bin' directory.
def _mk_ocaml_compiler(ctx: AnalysisContext, env: dict[str, ""], build_mode: BuildMode.type) -> cmd_args:
    ocaml_toolchain = ctx.attrs._ocaml_toolchain[OCamlToolchainInfo]
    compiler = ocaml_toolchain.ocaml_compiler if _is_native(build_mode) else ocaml_toolchain.ocaml_bytecode_compiler
    script_name = "ocamlopt" + build_mode.value + ".sh"
    script_args = _mk_script(ctx, script_name, [compiler], env)
    return script_args

# A command initialized with flags common to all compiler commands.
def _compiler_cmd(ctx: AnalysisContext, compiler: cmd_args, cc: cmd_args) -> cmd_args:
    ocaml_toolchain = ctx.attrs._ocaml_toolchain[OCamlToolchainInfo]

    cmd = cmd_args(compiler)
    cmd.add("-g", "-noautolink")
    if ocaml_toolchain.interop_includes:
        cmd.add("-nostdlib")
    cmd.add("-cc", cc)

    # First add compiler flags. These contain 'ocaml_common.bzl' flags
    # e.g. -w @a -safe-string followed by any target specific flags
    # (like -ppx ... for example). Note that ALL warnings (modulo
    # safe-string) are enabled and marked as fatal by this.
    cmd.add(ctx.attrs.compiler_flags)

    # Now, add in `COMMON_OCAML_WARNING_FLAGS` (defined by
    # 'fbcode/tools/build/buck/gen_modes.py') e.g.
    # -4-29-35-41-42-44-45-48-50 to selective disable warnings.
    attr_warnings = ctx.attrs.warnings_flags if ctx.attrs.warnings_flags != None else ""
    cmd.add("-w", ocaml_toolchain.warnings_flags + attr_warnings)

    return cmd

# The include paths for the immediate dependencies of the current target.
def _include_paths_in_context(ctx: AnalysisContext, build_mode: BuildMode.type):
    ocaml_libs = merge_ocaml_link_infos(_attr_deps_ocaml_link_infos(ctx)).info
    includes = []
    for lib in ocaml_libs:
        includes.extend(lib.include_dirs_nat if _is_native(build_mode) else lib.include_dirs_byt)

    # It's helpful if the stdlib search path comes last.
    ocaml_toolchain = ctx.attrs._ocaml_toolchain[OCamlToolchainInfo]
    includes.extend([cmd_args(ocaml_toolchain.interop_includes)] if ocaml_toolchain.interop_includes else [])

    return includes

def _compiler_flags(ctx: AnalysisContext, build_mode: BuildMode.type):
    ocaml_toolchain = ctx.attrs._ocaml_toolchain[OCamlToolchainInfo]
    mode_flags = ocaml_toolchain.ocamlopt_flags if _is_native(build_mode) else ocaml_toolchain.ocamlc_flags

    return ocaml_toolchain.ocaml_compiler_flags + mode_flags

# Configure a new compile command. Each source file (.mli, .ml) gets one of its
# own.
def _compile_cmd(ctx: AnalysisContext, compiler: cmd_args, build_mode: BuildMode.type, cc: cmd_args, includes: list[cmd_args]) -> cmd_args:
    cmd = _compiler_cmd(ctx, compiler, cc)
    cmd.add("-bin-annot")  # TODO(sf, 2023-02-21): Move this to 'gen_modes.py'?
    cmd.add(_compiler_flags(ctx, build_mode))
    cmd.add(cmd_args(includes, format = "-I={}"))

    return cmd

# Run any preprocessors, returning a list of ml/mli/c artifacts you can compile
def _preprocess(ctx: AnalysisContext, srcs: list["artifact"], build_mode: BuildMode.type) -> list["artifact"]:
    ocaml_toolchain = ctx.attrs._ocaml_toolchain[OCamlToolchainInfo]
    ocamllex = ocaml_toolchain.lex_compiler
    menhir = ocaml_toolchain.menhir_compiler  # We no longer use yacc_compiler, just menhir.

    result = []
    gen_dir = "_" + build_mode.value + "_gen_/"
    for src in srcs:
        ext = src.extension

        if ext == ".mly":
            name = gen_dir + paths.replace_extension(src.short_path, "")

            # We don't actually need the file `prefix`. It's a device
            # we use to get the `-b` flag argument.
            prefix = ctx.actions.write(name, "")
            parser = ctx.actions.declare_output(name + ".ml")
            parser_sig = ctx.actions.declare_output(name + ".mli")
            result.extend((parser_sig, parser))

            cmd = cmd_args([menhir, "--fixed-exception", "-b", cmd_args(prefix).ignore_artifacts(), src])
            cmd.hidden(parser.as_output(), parser_sig.as_output())
            ctx.actions.run(cmd, category = "ocaml_yacc_" + build_mode.value, identifier = src.short_path)

        elif ext == ".mll":
            name = gen_dir + paths.replace_extension(src.short_path, "")
            lexer = ctx.actions.declare_output(name + ".ml")
            result.append(lexer)

            cmd = cmd_args([ocamllex, src, "-o", lexer.as_output()])
            ctx.actions.run(cmd, category = "ocaml_lex_" + build_mode.value, identifier = src.short_path)

        else:
            result.append(src)

    return result

# Generate the dependencies
def _depends(ctx: AnalysisContext, srcs: list["artifact"], build_mode: BuildMode.type) -> "artifact":
    ocaml_toolchain = ctx.attrs._ocaml_toolchain[OCamlToolchainInfo]
    ocamldep = ocaml_toolchain.dep_tool

    dep_output_filename = "ocamldep_" + build_mode.value + ".mk"
    dep_output = ctx.actions.declare_output(dep_output_filename)
    dep_cmdline = cmd_args([ocamldep, "-native"])  # Yes, always native (see D36426635 for details).

    # We are writing the command into a file for later execution. Each flag
    # needs enclosing in quotes (since it's possible that some flags contain
    # might contain whitespace e.g. `foo --as-ppx`).
    dep_cmdline.add([cmd_args(f, format = "\"{}\"") for f in ctx.attrs.ocamldep_flags])

    # These -I's are for ocamldep.
    dep_cmdline.add(cmd_args([cmd_args(src).parent() for src in srcs], format = "-I {}"))
    dep_cmdline.add(srcs)
    dep_script_name = "ocamldep_" + build_mode.value + ".sh"
    dep_sh, _ = ctx.actions.write(
        dep_script_name,
        ["#!/usr/bin/env bash", cmd_args([dep_cmdline, ">", dep_output], delimiter = " ")],
        is_executable = True,
        allow_args = True,
    )
    ctx.actions.run(cmd_args(dep_sh).hidden(dep_output.as_output(), dep_cmdline), category = "ocamldep_" + build_mode.value)
    return dep_output

# Compile all the context's sources. If bytecode compiling, 'cmxs' & 'objs' will
# be empty in the returned tuple while 'cmos' will be non-empty. If compiling
# native code, 'cmos' in the returned info will be empty while 'objs' & 'cmxs'
# will be non-empty.
def _compile(ctx: AnalysisContext, compiler: cmd_args, build_mode: BuildMode.type) -> CompileResultInfo.type:
    opaque_enabled = "-opaque" in _compiler_flags(ctx, build_mode)
    is_native = _is_native(build_mode)
    is_bytecode = not is_native

    # Preprocess: Generate modules from lexers and parsers.
    srcs = _preprocess(ctx, ctx.attrs.srcs, build_mode)
    headers = [s for s in srcs if s.extension == ".h"]
    mlis = {s.short_path: s for s in srcs if s.extension == ".mli"}

    # 'ocamldep' will be sorting .cmo files or it will be sorting .cmx files and
    # so needs to know if we are byte or native compiling.
    depends_output = _depends(ctx, srcs, build_mode)

    # Compile
    produces = {}  # A tuple of things each source file produces.
    includes = {}  # Source file, .cmi pairs.
    stbs, objs, cmis, cmos, cmxs, cmts, cmtis, ppmlis, ppmls = ([], [], [], [], [], [], [], [], [])
    obj_dir = "_" + build_mode.value + "obj_/"
    for src in srcs:
        obj_name = obj_dir + paths.replace_extension(src.short_path, "")
        ext = src.extension

        if ext == ".mli":
            cmi = ctx.actions.declare_output(obj_name + ".cmi")
            cmti = ctx.actions.declare_output(obj_name + ".cmti")
            ppmli = ctx.actions.declare_output(obj_name + ".pp.mli") if build_mode.value == "expand" else None
            produces[src] = (cmi, cmti, ppmli)
            includes[src] = cmi
            cmis.append(cmi)
            cmtis.append(cmti)
            if ppmli != None:
                ppmlis.append(ppmli)

        elif ext == ".ml":
            # Sometimes a .ml file has an explicit .mli, sometimes its implicit
            # and we generate it. The variable below contains the artifact of
            # the explicit mli if present.
            mli = mlis.get(paths.replace_extension(src.short_path, ".mli"), None)

            cmt = ctx.actions.declare_output(obj_name + ".cmt")
            obj = ctx.actions.declare_output(obj_name + ".o") if is_native else None
            cmx = ctx.actions.declare_output(obj_name + ".cmx") if is_native else None
            cmo = ctx.actions.declare_output(obj_name + ".cmo") if is_bytecode else None
            cmi = ctx.actions.declare_output(obj_name + ".cmi") if mli == None else None
            ppml = ctx.actions.declare_output(obj_name + ".pp.ml") if build_mode.value == "expand" else None
            produces[src] = (obj, cmo, cmx, cmt, cmi, ppml)

            if cmo != None:
                cmos.append(cmo)
            if cmx != None:
                cmxs.append(cmx)
            if obj != None:
                objs.append(obj)
            if cmi != None:
                cmis.append(cmi)
                includes[src] = cmi
            if ppml != None:
                ppmls.append(ppml)
            cmts.append(cmt)

        elif ext == ".c":
            stb = ctx.actions.declare_output(obj_name + ".o")
            produces[src] = (stb,)
            stbs.append(stb)

        elif ext == ".h":
            pass

        else:
            fail("Unexpected extension: '" + src.basename + "'")

    # FIXME: Should populate these
    todo_inputs = []

    outputs = []
    for x in produces.values():
        outputs.extend(x)
    outputs = filter(None, outputs)

    # A file containing topologically sorted .cmx or .cmo files. We use the name
    # 'cmxs_order' without regard for which.
    cmxs_order = ctx.actions.declare_output("cmxs_order_" + build_mode.value + ".lst")

    pre = cxx_merge_cpreprocessors(ctx, [], filter(None, [d.get(CPreprocessorInfo) for d in _attr_deps(ctx)]))
    pre_args = pre.set.project_as_args("args")
    cc_sh_filename = "cc_" + build_mode.value + ".sh"
    cc = _mk_cc(ctx, [pre_args], cc_sh_filename)

    # These -I's are common to all compile commands for the given 'ctx'. This
    # includes the compiler include path.
    global_include_paths = _include_paths_in_context(ctx, build_mode)

    def f(ctx: AnalysisContext, artifacts, outputs):
        # A pair of mappings that detail which source files depend on which. See
        # [Note: Dynamic dependencies] in 'makefile.bzl'.
        makefile, makefile2 = parse_makefile(artifacts[depends_output].read_string(), srcs, opaque_enabled)

        # Ensure all '.ml' files are in the makefile, with zero dependencies if
        # necessary (so 'post_order_traversal' finds them).
        for x in srcs:
            if x.short_path.endswith(".ml"):
                if x not in makefile:
                    makefile[x] = []
                if x not in makefile2:
                    makefile2[x] = []

        mk_out = lambda x: outputs[x].as_output()

        # We want to write out all the compiled module files in transitive
        # dependency order.
        #
        # If compiling bytecode we order .cmo files (index 1) otherwise .cmx
        # files (index 2).
        cm_kind_index = 1 if is_bytecode else 2
        ctx.actions.write(
            mk_out(cmxs_order),  # write the ordered list
            [produces[x][cm_kind_index] for x in post_order_traversal(makefile) if x.short_path.endswith(".ml")],
        )

        # Compile
        for src in srcs:
            ext = src.extension

            # Things that are produced/includable from my dependencies
            depends_produce = []

            # These -I's are for the compile command for 'src'. They result from
            # the dependency of 'src' on other files in 'srcs'.
            depends_include_paths = []
            seen_dirs = {}
            for d in breadth_first_traversal(makefile2, makefile2.get(src, [])):
                # 'src' depends on 'd' (e.g. src='quux.ml' depends on
                # d='quux.mli').
                #
                # What artifacts does compiling 'd' produce? These are hidden
                # dependencies of the command to compile 'src' (e.g.
                # 'quux.cmi').
                #
                # In the event `-opaque` is enabled, 'makefile2' is a rewrite of
                # 'makefile' such that if 'f.mli' exists, then we will never
                # have a dependency here on 'f.cmx' ('f.cmo') only 'f.cmi'.
                depends_produce.extend(filter(None, produces[d]))
                i = includes.get(d, None)
                if i != None:
                    p = paths.dirname(i.short_path)
                    if not p in seen_dirs:
                        depends_include_paths.append(cmd_args(i).parent())
                        seen_dirs[p] = None

            # *All* the include paths needed to compile 'src'.
            all_include_paths = depends_include_paths + global_include_paths

            if ext == ".mli":
                (cmi, cmti, ppmli) = produces[src]
                cmd = _compile_cmd(ctx, compiler, build_mode, cc, all_include_paths)
                cmd.add(src, "-c", "-o", mk_out(cmi))
                if build_mode.value == "expand":
                    cmd.add("-dsource")
                cmd.hidden(mk_out(cmti), depends_produce)

                if build_mode.value == "expand":
                    sh = cmd_args(["/bin/sh", "-c", '"$@" 2> "$preprocessed_source_file"', "--", cmd])
                    env = {"preprocessed_source_file": mk_out(ppmli)}
                    ctx.actions.run(sh, category = "shell_ocaml_" + build_mode.value, identifier = src.short_path, env = env)
                else:
                    ctx.actions.run(cmd, category = "ocaml_compile_mli_" + build_mode.value, identifier = src.short_path)

            elif ext == ".ml":
                (obj, cmo, cmx, cmt, cmi, ppml) = produces[src]
                cmd = _compile_cmd(ctx, compiler, build_mode, cc, all_include_paths)
                cmd.hidden(depends_produce)
                if cmo != None:
                    cmd.add(src, "-c", "-o", mk_out(cmo))
                if cmx != None:
                    cmd.add(src, "-c", "-o", mk_out(cmx))
                cmd.hidden(mk_out(cmt))
                if build_mode.value == "expand":
                    cmd.add("-dsource")
                if obj != None:
                    cmd.hidden(mk_out(obj))
                if cmi != None:
                    cmd.add("-intf-suffix", ",nomli,")  # ignore any .mlis that aren't explicit dependencies
                    cmd.hidden(mk_out(cmi))
                else:
                    # An explicit '.mli' for this '.ml' is a dependency.
                    cmd.hidden(mlis[paths.replace_extension(src.short_path, ".mli")])

                if build_mode.value == "expand":
                    sh = cmd_args(["/bin/sh", "-c", '"$@" 2> "$preprocessed_source_file"', "--", cmd])
                    env = {"preprocessed_source_file": mk_out(ppml)}
                    ctx.actions.run(sh, category = "shell_ocaml_" + build_mode.value, identifier = src.short_path, env = env)
                else:
                    ctx.actions.run(cmd, category = "ocaml_compile_ml_" + build_mode.value, identifier = src.short_path)

            elif ext == ".c":
                (stb,) = produces[src]
                cmd = _compile_cmd(ctx, compiler, build_mode, cc, all_include_paths)

                # `ocaml_object` breaks for `-flto=...` so ensure `-fno-lto` prevails here.
                cmd.add(src, "-c", "-ccopt", "-fno-lto", "-ccopt", cmd_args(mk_out(stb), format = "-o \"{}\""))
                cmd.hidden(headers)  # Any .h files given are dependencies.
                ctx.actions.run(cmd, category = "ocaml_compile_c", identifier = src.short_path)

            elif ext == ".h":
                pass

            else:
                fail("Unexpected extension: '" + src.basename + "'")

    if outputs == []:
        ctx.actions.write(cmxs_order, "")
    else:
        ctx.actions.dynamic_output(dynamic = [depends_output], inputs = todo_inputs, outputs = outputs + [cmxs_order], f = f)

    return CompileResultInfo(cmxs_order = cmxs_order, stbs = stbs, objs = objs, cmis = cmis, cmos = cmos, cmxs = cmxs, cmts = cmts, cmtis = cmtis, ppmlis = ppmlis, ppmls = ppmls)

# The include path directories a client will provide a compile command to use
# the given artifacts.
def _include_paths(cmis: list["artifact"], cmos: list["artifact"]) -> cmd_args.type:
    include_paths = []
    seen_dirs = {}
    for f in cmis:
        p = paths.dirname(f.short_path)
        if not p in seen_dirs:
            include_paths.append(cmd_args(f).parent())
            seen_dirs[p] = None
    for f in cmos:
        p = paths.dirname(f.short_path)
        if not p in seen_dirs:
            include_paths.append(cmd_args(f).parent())
            seen_dirs[p] = None
    include_paths = cmd_args(include_paths)
    include_paths.hidden(cmis + cmos)
    return include_paths

def ocaml_library_impl(ctx: AnalysisContext) -> list["provider"]:
    opaque_enabled_nat = "-opaque" in _compiler_flags(ctx, BuildMode("native"))
    opaque_enabled_byt = "-opaque" in _compiler_flags(ctx, BuildMode("bytecode"))

    env = _mk_env(ctx)
    ocamlopt = _mk_ocaml_compiler(ctx, env, BuildMode("native"))
    ocamlc = _mk_ocaml_compiler(ctx, env, BuildMode("bytecode"))

    ld_nat = _mk_ld(ctx, [], "ld_native.sh")
    ld_byt = _mk_ld(ctx, [], "ld_bytecode.sh")

    cmd_nat = _compiler_cmd(ctx, ocamlopt, ld_nat)
    cmd_byt = _compiler_cmd(ctx, ocamlc, ld_byt)

    cmxs_order, stbs_nat, objs, cmis_nat, _cmos, cmxs, cmts_nat, cmtis_nat, _, _ = _compile_result_to_tuple(_compile(ctx, ocamlopt, BuildMode("native")))
    _, _, _, _, _, _, _, _, ppmlis, ppmls = _compile_result_to_tuple(_compile(ctx, ocamlopt, BuildMode("expand")))
    cmd_nat.add("-a")
    cmxa = ctx.actions.declare_output("lib" + ctx.attrs.name + ".cmxa")
    cmd_nat.add("-o", cmxa.as_output())
    if len([s for s in ctx.attrs.srcs if s.extension == ".ml"]) != 0:
        native_c_lib = ctx.actions.declare_output("lib" + ctx.attrs.name + ".a")
        cmd_nat.hidden(native_c_lib.as_output())
        native_c_libs = [native_c_lib]
    else:
        native_c_libs = []
    cmd_nat.add(stbs_nat, "-args", cmxs_order)

    # Native clients need these compile flags to use this library.
    include_paths_nat = _include_paths(cmis_nat, cmxs if not opaque_enabled_nat else [])

    # These were produced by the compile step and so are hidden dependencies of
    # the archive step.
    cmd_nat.hidden(cmxs, cmis_nat, objs, cmts_nat, cmtis_nat)
    ctx.actions.run(cmd_nat, category = "ocaml_archive_native")

    cmxs_order, stbs_byt, _objs, cmis_byt, cmos, _cmxs, cmts_byt, cmtis_byt, _ppmlis, _ppmls = _compile_result_to_tuple(_compile(ctx, ocamlc, BuildMode("bytecode")))
    cmd_byt.add("-a")

    cma = ctx.actions.declare_output("lib" + ctx.attrs.name + ".cma")
    cmd_byt.add("-o", cma.as_output())
    cmd_byt.add(stbs_byt, "-args", cmxs_order)

    # Bytecode clients need these compile flags to use this library.
    include_paths_byt = _include_paths(cmis_byt, cmos if not opaque_enabled_byt else [])

    # These were produced by the compile step and so are hidden dependencies of
    # the archive step.
    cmd_byt.hidden(cmos, cmis_byt, cmts_byt, cmtis_byt)
    ctx.actions.run(cmd_byt, category = "ocaml_archive_bytecode")

    infos = _attr_deps_ocaml_link_infos(ctx)
    infos.append(
        OCamlLinkInfo(info = [OCamlLibraryInfo(
            name = ctx.attrs.name,
            target = ctx.label,
            c_libs = [],
            stbs_nat = stbs_nat,
            stbs_byt = stbs_byt,
            cmas = [cma],
            cmxas = [cmxa],
            cmis_nat = cmis_nat,
            cmis_byt = cmis_byt,
            cmos = cmos,
            cmxs = cmxs,
            cmts_nat = cmts_nat,
            cmts_byt = cmts_byt,
            cmtis_nat = cmtis_nat,
            cmtis_byt = cmtis_byt,
            include_dirs_nat = [include_paths_nat],
            include_dirs_byt = [include_paths_byt],
            native_c_libs = native_c_libs,
            bytecode_c_libs = [],
        )]),
    )

    other_outputs = {
        "bytecode": cmis_byt + cmos,
        "expand": ppmlis + ppmls,
        "ide": cmis_nat + cmtis_nat + cmts_nat,
    }
    other_outputs_info = merge_other_outputs_info(ctx, other_outputs, _attr_deps_other_outputs_infos(ctx))

    info_ide = [
        DefaultInfo(
            default_output = cmxa,
            other_outputs = [cmd_args(other_outputs_info.info.project_as_args("ide"))],
        ),
    ]
    info_byt = [
        DefaultInfo(
            default_output = cma,
            other_outputs = [cmd_args(other_outputs_info.info.project_as_args("bytecode"))],
        ),
    ]
    info_expand = [
        DefaultInfo(
            default_output = cmxa,
            other_outputs = [cmd_args(other_outputs_info.info.project_as_args("expand"))],
        ),
    ]
    sub_targets = {"bytecode": info_byt, "expand": info_expand, "ide": info_ide}

    if ctx.attrs.bytecode_only:
        return info_byt

    return [
        DefaultInfo(default_output = cmxa, sub_targets = sub_targets),
        merge_ocaml_link_infos(infos),
        merge_link_infos(ctx, _attr_deps_merged_link_infos(ctx)),
        merge_shared_libraries(ctx.actions, deps = filter_and_map_idx(SharedLibraryInfo, _attr_deps(ctx))),
        merge_link_group_lib_info(deps = _attr_deps(ctx)),
        other_outputs_info,
        create_linkable_graph(
            ctx,
            deps = _attr_deps(ctx),
        ),
    ]

def ocaml_binary_impl(ctx: AnalysisContext) -> list["provider"]:
    ocaml_toolchain = ctx.attrs._ocaml_toolchain[OCamlToolchainInfo]

    env = _mk_env(ctx)
    ocamlopt = _mk_ocaml_compiler(ctx, env, BuildMode("native"))
    ocamlc = _mk_ocaml_compiler(ctx, env, BuildMode("bytecode"))

    link_infos = merge_link_infos(
        ctx,
        _attr_deps_merged_link_infos(ctx) + filter(None, [ocaml_toolchain.libc]),
    )
    ld_args, linker_deps, _ = make_link_args(ctx, [get_link_args(link_infos, LinkStyle("static_pic"))])
    ld_nat = _mk_ld(ctx, [ld_args], "ld_native.sh")
    ld_byt = _mk_ld(ctx, [ld_args], "ld_bytecode.sh")

    cmd_nat = _compiler_cmd(ctx, ocamlopt, ld_nat)
    cmd_byt = _compiler_cmd(ctx, ocamlc, ld_byt)

    # These -I's are to find 'stdlib.cmxa'/'stdlib.cma'.
    if ocaml_toolchain.interop_includes:
        cmd_nat.add(cmd_args(ocaml_toolchain.interop_includes, format = "-I={}"))
        cmd_byt.add(cmd_args(ocaml_toolchain.interop_includes, format = "-I={}"))

    for lib in merge_ocaml_link_infos(_attr_deps_ocaml_link_infos(ctx)).info:
        cmd_nat.add(lib.cmxas, lib.c_libs, lib.native_c_libs, lib.stbs_nat)
        cmd_byt.add(lib.cmas, lib.c_libs, lib.bytecode_c_libs, lib.stbs_byt)

    cmxs_order, stbs_nat, objs, cmis_nat, _cmos, cmxs, cmts_nat, cmtis_nat, _, _ = _compile_result_to_tuple(_compile(ctx, ocamlopt, BuildMode("native")))
    _, _, _, _, _, _, _, _, ppmlis, ppmls = _compile_result_to_tuple(_compile(ctx, ocamlopt, BuildMode("expand")))
    cmd_nat.add(stbs_nat, "-args", cmxs_order)

    # These were produced by the compile step and are therefore hidden
    # dependencies of the link step.
    cmd_nat.hidden(cmxs, cmis_nat, cmts_nat, cmtis_nat, objs, linker_deps)
    binary_nat = ctx.actions.declare_output(ctx.attrs.name + ".opt")
    cmd_nat.add("-cclib", "-lpthread")
    cmd_nat.add("-o", binary_nat.as_output())
    local_only = link_cxx_binary_locally(ctx)
    ctx.actions.run(cmd_nat, category = "ocaml_link_native", local_only = local_only)

    cmxs_order, stbs_byt, _objs, cmis_byt, cmos, _cmxs, cmts_byt, cmtis_byt, _, _ = _compile_result_to_tuple(_compile(ctx, ocamlc, BuildMode("bytecode")))
    cmd_byt.add(stbs_byt, "-args", cmxs_order)

    # These were produced by the compile step and are therefore hidden
    # dependencies of the link step.
    cmd_byt.hidden(cmos, cmis_byt, cmts_byt, cmtis_byt, linker_deps)
    binary_byt = ctx.actions.declare_output(ctx.attrs.name)
    cmd_byt.add("-custom")
    cmd_byt.add("-cclib", "-lpthread")
    cmd_byt.add("-o", binary_byt.as_output())
    local_only = link_cxx_binary_locally(ctx)
    ctx.actions.run(cmd_byt, category = "ocaml_link_bytecode", local_only = local_only)

    other_outputs = {
        "bytecode": cmis_byt + cmos,
        "expand": ppmlis + ppmls,
        "ide": cmis_nat + cmtis_nat + cmts_nat,
    }
    other_outputs_info = merge_other_outputs_info(ctx, other_outputs, _attr_deps_other_outputs_infos(ctx))

    info_ide = [
        DefaultInfo(
            default_output = binary_nat,
            other_outputs = [cmd_args(other_outputs_info.info.project_as_args("ide"))],
        ),
    ]
    info_expand = [
        DefaultInfo(
            default_output = binary_nat,
            other_outputs = [cmd_args(other_outputs_info.info.project_as_args("expand"))],
        ),
    ]
    info_byt = [
        DefaultInfo(
            default_output = binary_byt,
            other_outputs = [cmd_args(other_outputs_info.info.project_as_args("bytecode"))],
        ),
        RunInfo(args = [binary_byt]),
    ]
    sub_targets = {"bytecode": info_byt, "expand": info_expand, "ide": info_ide}

    if ctx.attrs.bytecode_only:
        return info_byt

    return [
        DefaultInfo(default_output = binary_nat, sub_targets = sub_targets),
        RunInfo(args = [binary_nat]),
    ]

def ocaml_object_impl(ctx: AnalysisContext) -> list["provider"]:
    ocaml_toolchain = ctx.attrs._ocaml_toolchain[OCamlToolchainInfo]

    env = _mk_env(ctx)
    ocamlopt = _mk_ocaml_compiler(ctx, env, BuildMode("native"))
    deps_link_info = merge_link_infos(ctx, _attr_deps_merged_link_infos(ctx))
    ld_args, linker_deps, _ = make_link_args(ctx, [get_link_args(deps_link_info, LinkStyle("static_pic"))])
    ld = _mk_ld(ctx, [ld_args], "ld.sh")

    cmxs_order, stbs, objs, cmis, _cmos, cmxs, cmts, cmtis, _, _ = _compile_result_to_tuple(_compile(ctx, ocamlopt, BuildMode("native")))
    _, _, _, _, _, _, _, _, ppmlis, ppmls = _compile_result_to_tuple(_compile(ctx, ocamlopt, BuildMode("expand")))

    cmd = _compiler_cmd(ctx, ocamlopt, ld)

    # These -I's are to find 'stdlib.cmxa'/'stdlib.cma'.
    if ocaml_toolchain.interop_includes:
        cmd.add(cmd_args(ocaml_toolchain.interop_includes, format = "-I={}"))

    for lib in merge_ocaml_link_infos(_attr_deps_ocaml_link_infos(ctx)).info:
        cmd.add(lib.cmxas, lib.c_libs, lib.native_c_libs, lib.stbs_nat)
        cmd.hidden(lib.cmxs, lib.cmis_nat, lib.cmts_nat)

    cmd.add(stbs, "-args", cmxs_order)
    cmd.hidden(cmxs, cmis, cmts, objs, cmtis, linker_deps)

    obj = ctx.actions.declare_output(ctx.attrs.name + ".o")
    cmd.add("-output-complete-obj")
    cmd.add("-o", obj.as_output())
    local_only = link_cxx_binary_locally(ctx)
    ctx.actions.run(cmd, category = "ocaml_complete_obj_link", local_only = local_only)

    cxx_toolchain = get_cxx_toolchain_info(ctx)
    linker_type = cxx_toolchain.linker_info.type
    link_infos = {}
    for link_style in LinkStyle:
        link_infos[link_style] = LinkInfos(default = LinkInfo(
            linkables = [
                ObjectsLinkable(objects = [obj], linker_type = linker_type),
            ],
        ))
    obj_link_info = create_merged_link_info(
        ctx,
        pic_behavior = cxx_toolchain.pic_behavior,
        link_infos = link_infos,
        exported_deps = [deps_link_info],
    )

    other_outputs = {
        "bytecode": [],
        "expand": ppmlis + ppmls,
        "ide": cmis + cmtis + cmts,
    }
    other_outputs_info = merge_other_outputs_info(ctx, other_outputs, _attr_deps_other_outputs_infos(ctx))

    info_ide = [
        DefaultInfo(
            default_output = obj,
            other_outputs = [cmd_args(other_outputs_info.info.project_as_args("ide"))],
        ),
    ]
    info_byt = [
        DefaultInfo(
            default_output = obj,
            other_outputs = [cmd_args(other_outputs_info.info.project_as_args("bytecode"))],
        ),
    ]
    info_expand = [
        DefaultInfo(
            default_output = obj,
            other_outputs = [cmd_args(other_outputs_info.info.project_as_args("expand"))],
        ),
    ]
    sub_targets = {"bytecode": info_byt, "expand": info_expand, "ide": info_ide}

    return [
        DefaultInfo(default_output = obj, sub_targets = sub_targets),
        obj_link_info,
        merge_link_group_lib_info(deps = _attr_deps(ctx)),
        merge_shared_libraries(ctx.actions, deps = filter_and_map_idx(SharedLibraryInfo, _attr_deps(ctx))),
        create_linkable_graph(
            ctx,
            deps = ctx.attrs.deps,
        ),
    ]

# `ocaml_shared` enables one to produce an OCaml "plugin". Such native code
# ('.cmxs') files can be dynamically loaded into a running-process via the
# `Dynlink` module. Example use cases include writing compiler plugins for use
# with the `-plugin` compiler flag & "deriver" plugins for use with the
# `ppx_deriving` framework.
def ocaml_shared_impl(ctx: AnalysisContext) -> list["provider"]:
    ocaml_toolchain = ctx.attrs._ocaml_toolchain[OCamlToolchainInfo]

    env = _mk_env(ctx)
    ocamlopt = _mk_ocaml_compiler(ctx, env, BuildMode("native"))

    link_infos = merge_link_infos(
        ctx,
        _attr_deps_merged_link_infos(ctx) + filter(None, [ocaml_toolchain.libc]),
    )
    ld_args, linker_deps, _ = make_link_args(ctx, [get_link_args(link_infos, LinkStyle("static_pic"))])

    # 'ocamlopt.opt' with '-cc' fails to propagate '-shared' (and potentially
    # other required flags - see the darwin "dylib" specific block below) to the
    # linker. See https://www.internalfb.com/phabricator/paste/view/P596226070.
    # This is a workaround.
    shared_args = ["-shared"]
    if host_info().os.is_macos:
        shared_args.extend(["-flat_namespace", "-undefined suppress", "-Wl,-no_compact_unwind"])

    ld_nat = _mk_ld(ctx, shared_args + [ld_args], "ld_native.sh")

    cmd_nat = _compiler_cmd(ctx, ocamlopt, ld_nat)

    cmxs_order, stbs, objs, cmis_nat, _cmos, cmxs, cmts_nat, cmtis_nat, _, _ = _compile_result_to_tuple(_compile(ctx, ocamlopt, BuildMode("native")))
    _, _, _, _, _, _, _, _, ppmlis, ppmls = _compile_result_to_tuple(_compile(ctx, ocamlopt, BuildMode("expand")))

    cmd_nat.add(stbs, "-args", cmxs_order)

    # These were produced by the compile step and are therefore hidden
    # dependencies of the link step.
    cmd_nat.hidden(cmxs, cmis_nat, cmts_nat, cmtis_nat, objs, linker_deps)
    binary_nat = ctx.actions.declare_output(ctx.attrs.name + ".cmxs")
    cmd_nat.add("-shared")
    cmd_nat.add("-o", binary_nat.as_output())
    local_only = link_cxx_binary_locally(ctx)
    ctx.actions.run(cmd_nat, category = "ocaml_shared_link", local_only = local_only)

    other_outputs = {
        "bytecode": [],
        "expand": ppmlis + ppmls,
        "ide": cmis_nat + cmtis_nat + cmts_nat,
    }
    other_outputs_info = merge_other_outputs_info(ctx, other_outputs, _attr_deps_other_outputs_infos(ctx))

    info_ide = [
        DefaultInfo(
            default_output = binary_nat,
            other_outputs = [cmd_args(other_outputs_info.info.project_as_args("ide"))],
        ),
    ]
    info_expand = [
        DefaultInfo(
            default_output = binary_nat,
            other_outputs = [cmd_args(other_outputs_info.info.project_as_args("expand"))],
        ),
    ]
    sub_targets = {"expand": info_expand, "ide": info_ide}

    return [
        DefaultInfo(default_output = binary_nat, sub_targets = sub_targets),
    ]

def prebuilt_ocaml_library_impl(ctx: AnalysisContext) -> list["provider"]:
    # examples:
    #   name: 'threads'
    #   bytecode_c_libs: 'libthreads.a'
    #   bytecode_lib: 'threads.cma'
    #   native_lib: 'threads.cmxa'
    #   c_libs: 'libcore_kernel_stubs.a'
    #   include_dir:  'share/dotopam/.../threads'
    #   lib_dir: ""
    #   native_c_libs: 'libthreadsnat.a'

    name = ctx.attrs.name
    c_libs = ctx.attrs.c_libs
    cmas = [ctx.attrs.bytecode_lib] if ctx.attrs.bytecode_lib != None else []
    cmxas = [ctx.attrs.native_lib] if ctx.attrs.native_lib != None else []

    # `ctx.attrs.include_dirs` has type `"artifact"`, convert it to a `cmd_args`
    include_dirs = [cmd_args(ctx.attrs.include_dir)] if ctx.attrs.include_dir != None else []
    native_c_libs = ctx.attrs.native_c_libs
    bytecode_c_libs = ctx.attrs.bytecode_c_libs

    info = OCamlLibraryInfo(
        name = name,
        target = ctx.label,
        c_libs = c_libs,
        cmas = cmas,
        cmxas = cmxas,
        include_dirs_nat = include_dirs,
        include_dirs_byt = include_dirs,
        stbs_nat = [],
        stbs_byt = [],
        cmis_nat = [],
        cmis_byt = [],
        cmos = [],
        cmxs = [],
        cmts_nat = [],
        cmts_byt = [],
        cmtis_nat = [],
        cmtis_byt = [],
        native_c_libs = native_c_libs,
        bytecode_c_libs = bytecode_c_libs,
    )

    native_infos, ocaml_infos = ([], [])
    for dep in ctx.attrs.deps:
        used = False
        if OCamlLinkInfo in dep:
            used = True
            ocaml_infos.append(dep[OCamlLinkInfo])
        if MergedLinkInfo in dep:
            used = True
            native_infos.append(dep[MergedLinkInfo])
        if PythonLibraryInfo in dep:
            used = True
        if not used:
            fail("Unexpected link info encountered")

    return [
        DefaultInfo(),
        merge_ocaml_link_infos(ocaml_infos + [OCamlLinkInfo(info = [info])]),
        merge_link_infos(ctx, native_infos),
        merge_link_group_lib_info(deps = ctx.attrs.deps),
        merge_shared_libraries(ctx.actions, deps = filter_and_map_idx(SharedLibraryInfo, ctx.attrs.deps)),
        create_linkable_graph(
            ctx,
            deps = ctx.attrs.deps,
        ),
    ]
