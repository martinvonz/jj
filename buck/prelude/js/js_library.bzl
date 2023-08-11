# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:paths.bzl", "paths")
load("@prelude//js:js_providers.bzl", "JsLibraryInfo", "get_transitive_outputs")
load("@prelude//js:js_utils.bzl", "TRANSFORM_PROFILES", "get_canonical_src_name", "get_flavors", "run_worker_commands")
load("@prelude//utils:utils.bzl", "expect", "map_idx")

# A group of sources that all have the same canonical name. The main_source is arbitrary but
# consistent (it is just the first source encountered when processing the src files).
GroupedSource = record(
    canonical_name = str,
    main_source = "artifact",
    additional_sources = ["artifact"],
)

def _get_grouped_srcs(ctx: AnalysisContext) -> list[GroupedSource.type]:
    grouped_srcs = {}
    for src in ctx.attrs.srcs:
        # TODO(ianc) also support sources with an "inner path".
        expect(
            type(src) == "artifact",
            "src {} is not an artifact, its type is: {}".format(src, type(src)),
        )
        canonical_src_name = get_canonical_src_name(src.short_path)
        if grouped_srcs.get(canonical_src_name, None) == None:
            grouped_srcs[canonical_src_name] = GroupedSource(
                canonical_name = canonical_src_name,
                main_source = src,
                additional_sources = [],
            )
        else:
            grouped_srcs[canonical_src_name].additional_sources.append(src)

    return grouped_srcs.values()

def _get_virtual_path(ctx: AnalysisContext, src: "artifact", base_path: [str, None]) -> str:
    package = ctx.label.package
    if base_path and base_path not in ["", "."]:
        package = paths.join(package, base_path)

    return paths.join(package, src.short_path)

def _build_js_files(
        ctx: AnalysisContext,
        transform_profile: str,
        flavors: list[str],
        grouped_srcs: list[GroupedSource.type]) -> list["artifact"]:
    if not grouped_srcs:
        return []

    all_output_paths = []
    all_command_args_files = []
    all_hidden_artifacts = []
    for grouped_src in grouped_srcs:
        identifier = "{}/{}".format(transform_profile, grouped_src.canonical_name)

        output_path = ctx.actions.declare_output(identifier)
        job_args = {
            "additionalSources": [{
                "sourcePath": additional_source,
                "virtualPath": _get_virtual_path(ctx, additional_source, ctx.attrs.base_path),
            } for additional_source in grouped_src.additional_sources],
            "command": "transform",
            "flavors": flavors,
            "outputFilePath": output_path,
            "release": ctx.attrs._is_release,
            "sourceJsFileName": _get_virtual_path(ctx, grouped_src.main_source, ctx.attrs.base_path),
            "sourceJsFilePath": grouped_src.main_source,
            "transformProfile": "default" if transform_profile == "transform-profile-default" else transform_profile,
        }
        if ctx.attrs.extra_json:
            job_args["extraData"] = cmd_args(ctx.attrs.extra_json, delimiter = "")

        command_args_file = ctx.actions.write_json(
            "{}_command_args".format(identifier),
            job_args,
        )

        all_output_paths.append(output_path)
        all_command_args_files.append(command_args_file)
        all_hidden_artifacts.append(cmd_args([output_path.as_output(), grouped_src.main_source] + grouped_src.additional_sources))

    batch_size = 25
    command_count = len(all_output_paths)
    for (batch_number, start_index) in enumerate(range(0, command_count, batch_size)):
        end_index = min(start_index + batch_size, command_count)
        run_worker_commands(
            ctx = ctx,
            worker_tool = ctx.attrs.worker,
            command_args_files = all_command_args_files[start_index:end_index],
            identifier = "{}_{}_batch{}".format(ctx.label.name, transform_profile, batch_number),
            category = "transform",
            hidden_artifacts = all_hidden_artifacts[start_index:end_index],
        )

    return all_output_paths

def _build_library_files(
        ctx: AnalysisContext,
        transform_profile: str,
        flavors: list[str],
        js_files: list["artifact"]) -> "artifact":
    output_path = ctx.actions.declare_output("{}/library_files".format(transform_profile))
    command_args_file = ctx.actions.write_json(
        "library_files_{}_command_args".format(transform_profile),
        {
            "command": "library-files",
            "flavors": flavors,
            "outputFilePath": output_path,
            "platform": ctx.attrs._platform,
            "release": ctx.attrs._is_release,
            "sourceFilePaths": js_files,
        },
    )

    run_worker_commands(
        ctx = ctx,
        worker_tool = ctx.attrs.worker,
        command_args_files = [command_args_file],
        identifier = transform_profile,
        category = "library_files",
        hidden_artifacts = [cmd_args([output_path.as_output()] + js_files)],
    )
    return output_path

def _build_js_library(
        ctx: AnalysisContext,
        transform_profile: str,
        library_files: "artifact",
        flavors: list[str],
        js_library_deps: list["artifact"]) -> "artifact":
    output_path = ctx.actions.declare_output("{}.jslib".format(transform_profile))
    job_args = {
        "aggregatedSourceFilesFilePath": library_files,
        "command": "library-dependencies",
        "dependencyLibraryFilePaths": js_library_deps,
        "flavors": flavors,
        "outputPath": output_path,
        "platform": ctx.attrs._platform,
        "release": ctx.attrs._is_release,
    }

    if ctx.attrs.extra_json:
        job_args["extraData"] = cmd_args(ctx.attrs.extra_json, delimiter = "")

    command_args_file = ctx.actions.write_json(
        "library_deps_{}_args".format(transform_profile),
        job_args,
    )

    run_worker_commands(
        ctx = ctx,
        worker_tool = ctx.attrs.worker,
        command_args_files = [command_args_file],
        identifier = transform_profile,
        category = "library_dependencies",
        hidden_artifacts = [cmd_args([
            output_path.as_output(),
            library_files,
        ] + js_library_deps)],
    )

    return output_path

def js_library_impl(ctx: AnalysisContext) -> list["provider"]:
    if ctx.attrs._build_only_native_code:
        sub_targets = {}
        unused_output = ctx.actions.write("unused.js", [])

        for transform_profile in TRANSFORM_PROFILES:
            sub_targets[transform_profile] = [
                DefaultInfo(default_output = unused_output),
                JsLibraryInfo(
                    output = unused_output,
                    transitive_outputs = None,
                ),
            ]

        return [
            DefaultInfo(default_output = None, sub_targets = sub_targets),
        ]

    grouped_srcs = _get_grouped_srcs(ctx)
    flavors = get_flavors(ctx)
    sub_targets = {}

    for transform_profile in TRANSFORM_PROFILES:
        built_js_files = _build_js_files(ctx, transform_profile, flavors, grouped_srcs)
        library_files = _build_library_files(ctx, transform_profile, flavors, built_js_files)

        js_library_deps = dedupe(map_idx(
            JsLibraryInfo,
            [dep[DefaultInfo].sub_targets[transform_profile] for dep in ctx.attrs.deps],
        ))
        js_library = _build_js_library(
            ctx,
            transform_profile,
            library_files,
            flavors,
            [js_library_dep.output for js_library_dep in js_library_deps],
        )

        transitive_outputs = get_transitive_outputs(
            ctx.actions,
            value = js_library,
            deps = js_library_deps,
        )

        sub_targets[transform_profile] = [
            DefaultInfo(default_output = js_library),
            JsLibraryInfo(
                output = js_library,
                transitive_outputs = transitive_outputs,
            ),
        ]

    return [
        DefaultInfo(default_output = None, sub_targets = sub_targets),
    ]
