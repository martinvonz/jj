# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# Implementation of the `genrule` build rule.

load("@prelude//:cache_mode.bzl", "CacheModeInfo")
load("@prelude//:genrule_local_labels.bzl", "genrule_labels_require_local")
load("@prelude//:genrule_toolchain.bzl", "GenruleToolchainInfo")
load("@prelude//:open_source.bzl", "is_open_source")
load("@prelude//os_lookup:defs.bzl", "OsLookup")
load("@prelude//utils:utils.bzl", "flatten", "value_or")

# Currently, some rules require running from the project root, so provide an
# opt-in list for those here.  Longer-term, these should be ported to actual
# rule implementations in v2, rather then using `genrule`s.
_BUILD_ROOT_LABELS = {label: True for label in [
    # The buck2 test suite
    "buck2_test_build_root",
    "antlir_macros",
    "rust_bindgen",
    "haskell_hsc",
    "cql_cxx_genrule",
    "clang-module",
    "cuda_build_root",
    "bundle_pch_genrule",  # Compiles C++, and so need to run from build root
    "lpm_package",
    "haskell_dll",
    "fnlc_build",
    "udf_sql",
    "redex_genrule",  # T148016945
    "pxl",  # T151533831
    "app_modules_genrule",  # produces JSON containing file paths that are read from the root dir.
]}

# In Buck1 the SRCS environment variable is only set if the substring SRCS is on the command line.
# That's a horrible heuristic, and doesn't account for users accessing $SRCS from a shell script.
# But in some cases, $SRCS is so large it breaks the process limit, so have a label to opt in to
# that behavior.
_NO_SRCS_ENVIRONMENT_LABEL = "no_srcs_environment"

def _requires_build_root(ctx: AnalysisContext) -> bool:
    for label in ctx.attrs.labels:
        if label in _BUILD_ROOT_LABELS:
            return True
    return False

def _requires_local(ctx: AnalysisContext) -> bool:
    return genrule_labels_require_local(ctx.attrs.labels)

def _ignore_artifacts(ctx: AnalysisContext) -> bool:
    return "buck2_ignore_artifacts" in ctx.attrs.labels

def _requires_no_srcs_environment(ctx: AnalysisContext) -> bool:
    return _NO_SRCS_ENVIRONMENT_LABEL in ctx.attrs.labels

# We don't want to use cache mode in open source because the config keys that drive it aren't wired up
_USE_CACHE_MODE = not is_open_source()

# Extra attributes required by every genrule based on genrule_impl
def genrule_attributes() -> dict[str, "attribute"]:
    attributes = {
        "metadata_env_var": attrs.option(attrs.string(), default = None),
        "metadata_path": attrs.option(attrs.string(), default = None),
        "no_outputs_cleanup": attrs.bool(default = False),
        "_genrule_toolchain": attrs.default_only(attrs.toolchain_dep(default = "toolchains//:genrule", providers = [GenruleToolchainInfo])),
    }

    if _USE_CACHE_MODE:
        # FIXME: prelude// should be standalone (not refer to fbsource//)
        attributes["_cache_mode"] = attrs.dep(default = "fbsource//xplat/buck2/platform/cache_mode:cache_mode")

    return attributes

def _get_cache_mode(ctx: AnalysisContext) -> CacheModeInfo.type:
    if _USE_CACHE_MODE:
        return ctx.attrs._cache_mode[CacheModeInfo]
    else:
        return CacheModeInfo(allow_cache_uploads = False, cache_bust_genrules = False)

def genrule_impl(ctx: AnalysisContext) -> list["provider"]:
    # Directories:
    #   sh - sh file
    #   src - sources files
    #   out - where outputs go
    # `src` is the current directory
    # Buck1 uses `.` as output, but that won't work since
    # Buck2 clears the output directory before execution, and thus src/sh too.
    return process_genrule(ctx, ctx.attrs.out, ctx.attrs.outs)

def _declare_output(ctx: AnalysisContext, path: str) -> "artifact":
    if path == ".":
        return ctx.actions.declare_output("out", dir = True)
    elif path.endswith("/"):
        return ctx.actions.declare_output("out", path[:-1], dir = True)
    else:
        return ctx.actions.declare_output("out", path)

def _project_output(out: "artifact", path: str) -> "artifact":
    if path == ".":
        return out
    elif path.endswith("/"):
        return out.project(path[:-1], hide_prefix = True)
    else:
        return out.project(path, hide_prefix = True)

def process_genrule(
        ctx: AnalysisContext,
        out_attr: [str, None],
        outs_attr: [dict, None],
        extra_env_vars: dict = {},
        identifier: [str, None] = None) -> list["provider"]:
    if (out_attr != None) and (outs_attr != None):
        fail("Only one of `out` and `outs` should be set. Got out=`%s`, outs=`%s`" % (repr(out_attr), repr(outs_attr)))

    local_only = _requires_local(ctx)

    # NOTE: Eventually we shouldn't require local_only here, since we should be
    # fine with caching local fallbacks if necessary (or maybe that should be
    # disallowed as a matter of policy), but for now let's be safe.
    cacheable = value_or(ctx.attrs.cacheable, True) and local_only

    # TODO(cjhopman): verify output paths are ".", "./", or forward-relative.
    if out_attr != None:
        out_env = out_attr
        out_artifact = _declare_output(ctx, out_attr)
        named_outputs = {}
        default_outputs = [out_artifact]
    elif outs_attr != None:
        out_env = ""
        out_artifact = ctx.actions.declare_output("out", dir = True)

        named_outputs = {
            name: [_project_output(out_artifact, path) for path in outputs]
            for (name, outputs) in outs_attr.items()
        }

        default_outputs = [
            _project_output(out_artifact, path)
            for path in (ctx.attrs.default_outs or [])
        ]
        if len(default_outputs) == 0:
            # We want building to force something to be built, so make sure it contains at least one artifact
            default_outputs = [out_artifact]
    else:
        fail("One of `out` or `outs` should be set. Got `%s`" % repr(ctx.attrs))

    # Some custom rules use `process_genrule` but doesn't set this attribute.
    is_windows = hasattr(ctx.attrs, "_exec_os_type") and ctx.attrs._exec_os_type[OsLookup].platform == "windows"
    if is_windows:
        path_sep = "\\"
        cmd = ctx.attrs.cmd_exe if ctx.attrs.cmd_exe != None else ctx.attrs.cmd
        if cmd == None:
            fail("One of `cmd` or `cmd_exe` should be set.")
    else:
        path_sep = "/"
        cmd = ctx.attrs.bash if ctx.attrs.bash != None else ctx.attrs.cmd
        if cmd == None:
            fail("One of `cmd` or `bash` should be set.")
    cmd = cmd_args(cmd)

    # For backwards compatibility with Buck1.
    if is_windows:
        # Replace $OUT and ${OUT}
        cmd.replace_regex("\\$(OUT\\b|\\{OUT\\})", "%OUT%")
        cmd.replace_regex("\\$(SRCDIR\\b|\\{SRCDIR\\})", "%SRCDIR%")
        cmd.replace_regex("\\$(SRCS\\b|\\{SRCS\\})", "%SRCS%")
        cmd.replace_regex("\\$(TMP\\b|\\{TMP\\})", "%TMP%")

    if _ignore_artifacts(ctx):
        cmd = cmd.ignore_artifacts()

    if type(ctx.attrs.srcs) == type([]):
        # FIXME: We should always use the short_path, but currently that is sometimes blank.
        # See fbcode//buck2/tests/targets/rules/genrule:genrule-dot-input for a test that exposes it.
        symlinks = {src.short_path: src for src in ctx.attrs.srcs}

        if len(symlinks) != len(ctx.attrs.srcs):
            for src in ctx.attrs.srcs:
                name = src.short_path
                if symlinks[name] != src:
                    msg = "genrule srcs include duplicative name: `{}`. ".format(name)
                    msg += "`{}` conflicts with `{}`".format(symlinks[name].owner, src.owner)
                    fail(msg)
    else:
        symlinks = ctx.attrs.srcs
    srcs_artifact = ctx.actions.symlinked_dir("srcs" if not identifier else "{}-srcs".format(identifier), symlinks)

    # Setup environment variables.
    srcs = cmd_args()
    for symlink in symlinks:
        srcs.add(cmd_args(srcs_artifact, format = path_sep.join([".", "{}", symlink.replace("/", path_sep)])))
    out_fmt = path_sep.join([".", "{}", "..", "out"])
    if out_env != "":
        out_fmt += path_sep + out_env.replace("/", path_sep)
    env_vars = {
        "ASAN_OPTIONS": cmd_args("detect_leaks=0,detect_odr_violation=0"),
        "GEN_DIR": cmd_args("GEN_DIR_DEPRECATED"),  # ctx.relpath(ctx.output_root_dir(), srcs_path)
        "OUT": cmd_args(srcs_artifact, format = out_fmt),
        "SRCDIR": cmd_args(srcs_artifact, format = path_sep.join([".", "{}"])),
        "SRCS": srcs,
    } | {k: cmd_args(v) for k, v in getattr(ctx.attrs, "env", {}).items()}

    # RE will cache successful actions that don't produce the desired outptuts,
    # so if that happens and _then_ we add a local-only label, we'll get a
    # cache hit on the action that didn't produce the outputs and get the error
    # again (thus making the label useless). So, when a local-only label is
    # set, we make the action *different*.
    if local_only:
        env_vars["__BUCK2_LOCAL_ONLY_CACHE_BUSTER"] = cmd_args("")

    # For now, when uploads are enabled, be safe and avoid sharing cache hits.
    cache_bust = _get_cache_mode(ctx).cache_bust_genrules

    if cacheable and cache_bust:
        env_vars["__BUCK2_ALLOW_CACHE_UPLOADS_CACHE_BUSTER"] = cmd_args("")

    if _requires_no_srcs_environment(ctx):
        env_vars.pop("SRCS")

    for key, value in extra_env_vars.items():
        env_vars[key] = value

    # Create required directories.
    if is_windows:
        script = [
            cmd_args(srcs_artifact, format = "if not exist .\\{}\\..\\out mkdir .\\{}\\..\\out"),
            cmd_args("if NOT \"%TEMP%\" == \"\" set \"TMP=%TEMP%\""),
        ]
        script_extension = "bat"
    else:
        script = [
            # Use a somewhat unique exit code so this can get retried on RE (T99656531).
            cmd_args(srcs_artifact, format = "mkdir -p ./{}/../out || exit 99"),
            cmd_args("export TMP=${TMPDIR:-/tmp}"),
        ]
        script_extension = "sh"

    # Actually define the operation, relative to where we changed to
    script.append(cmd)

    hidden = []
    genrule_toolchain = ctx.attrs._genrule_toolchain[GenruleToolchainInfo]
    zip_scrubber = genrule_toolchain.zip_scrubber
    if not is_windows and zip_scrubber != None:
        zip_outputs = [output for output in default_outputs + flatten(named_outputs.values()) if output.extension == ".zip"]

        if zip_outputs:
            hidden.append(zip_scrubber)

            # Any outputs that are .zip files need to be "scrubbed" to ensure that they are deterministic.
            script = [
                cmd_args("ORIGINAL_DIR_FOR_ZIP_SCRUBBING=$(pwd)"),
            ] + script + [
                cmd_args('cd "$ORIGINAL_DIR_FOR_ZIP_SCRUBBING"'),
            ] + [
                cmd_args(zip_scrubber, output, delimiter = " ", quote = "shell")
                for output in zip_outputs
            ]

    # Some rules need to run from the build root, but for everything else, `cd`
    # into the sandboxed source dir and relative all paths to that.
    if not _requires_build_root(ctx):
        srcs_dir = srcs_artifact
        if not is_windows:
            srcs_dir = cmd_args(srcs_dir, quote = "shell")
        script = (
            # Change to the directory that genrules expect.
            [cmd_args(srcs_dir, format = "cd {}")] +
            # Relative all paths in the command to the sandbox dir.
            [cmd.relative_to(srcs_artifact) for cmd in script]
        )

        # Relative all paths in the env to the sandbox dir.
        env_vars = {key: val.relative_to(srcs_artifact) for key, val in env_vars.items()}

    if is_windows:
        # Should be in the beginning.
        script = [cmd_args("@echo off")] + script

    sh_script, _ = ctx.actions.write(
        "sh/genrule.{}".format(script_extension) if not identifier else "sh/{}-genrule.{}".format(identifier, script_extension),
        script,
        is_executable = True,
        allow_args = True,
    )
    if is_windows:
        script_args = ["cmd.exe", "/v:off", "/c", sh_script]
    else:
        script_args = ["/usr/bin/env", "bash", "-e", sh_script]

    # Only set metadata arguments when they are non-null
    metadata_args = {}
    if ctx.attrs.metadata_env_var:
        metadata_args["metadata_env_var"] = ctx.attrs.metadata_env_var
    if ctx.attrs.metadata_path:
        metadata_args["metadata_path"] = ctx.attrs.metadata_path

    category = "genrule"
    if ctx.attrs.type != None:
        # As of 09/2021, all genrule types were legal snake case if their dashes and periods were replaced with underscores.
        category += "_" + ctx.attrs.type.replace("-", "_").replace(".", "_")
    ctx.actions.run(
        cmd_args(script_args).hidden([cmd, srcs_artifact, out_artifact.as_output()] + hidden),
        env = env_vars,
        local_only = local_only,
        allow_cache_upload = cacheable,
        category = category,
        identifier = identifier,
        no_outputs_cleanup = ctx.attrs.no_outputs_cleanup,
        **metadata_args
    )

    providers = [DefaultInfo(
        default_outputs = default_outputs,
        sub_targets = {k: [DefaultInfo(default_outputs = v)] for (k, v) in named_outputs.items()},
    )]

    # The cxx_genrule also forwards here, and that doesn't have .executable, so use getattr
    if getattr(ctx.attrs, "executable", False):
        providers.append(RunInfo(args = cmd_args(default_outputs)))
    return providers
