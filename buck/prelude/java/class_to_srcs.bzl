# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(
    "@prelude//java:java_toolchain.bzl",
    "JavaTestToolchainInfo",  # @unused Used as a type
    "JavaToolchainInfo",  # @unused Used as a type
)

def _class_to_src_map_args(mapping: ["artifact", None]):
    if mapping != None:
        return cmd_args(mapping)
    return cmd_args()

JavaClassToSourceMapTset = transitive_set(
    args_projections = {
        "class_to_src_map": _class_to_src_map_args,
    },
)

JavaClassToSourceMapInfo = provider(
    fields = [
        "tset",
    ],
)

def create_class_to_source_map_info(
        ctx: AnalysisContext,
        mapping: ["artifact", None] = None,
        deps = [Dependency]) -> JavaClassToSourceMapInfo.type:
    return JavaClassToSourceMapInfo(
        tset = ctx.actions.tset(
            JavaClassToSourceMapTset,
            value = mapping,
            children = [d[JavaClassToSourceMapInfo].tset for d in deps if JavaClassToSourceMapInfo in d],
        ),
    )

def create_class_to_source_map_from_jar(
        actions: "actions",
        name: str,
        java_toolchain: JavaToolchainInfo.type,
        jar: "artifact",
        srcs: list["artifact"]) -> "artifact":
    output = actions.declare_output(name)
    cmd = cmd_args(java_toolchain.gen_class_to_source_map[RunInfo])
    cmd.add("-o", output.as_output())
    cmd.add(jar)
    for src in srcs:
        cmd.add(cmd_args(src))
    actions.run(cmd, category = "class_to_srcs_map")
    return output

def merge_class_to_source_map_from_jar(
        actions: "actions",
        name: str,
        java_test_toolchain: JavaTestToolchainInfo.type,
        mapping: ["artifact", None] = None,
        relative_to: ["cell_root", None] = None,
        deps = [JavaClassToSourceMapInfo.type]) -> "artifact":
    output = actions.declare_output(name)
    cmd = cmd_args(java_test_toolchain.merge_class_to_source_maps[RunInfo])
    cmd.add(cmd_args(output.as_output(), format = "--output={}"))
    if relative_to != None:
        cmd.add(cmd_args(str(relative_to), format = "--relative-to={}"))
    tset = actions.tset(
        JavaClassToSourceMapTset,
        value = mapping,
        children = [d.tset for d in deps],
    )
    class_to_source_files = tset.project_as_args("class_to_src_map")
    mappings_file = actions.write("class_to_src_map.txt", class_to_source_files)
    cmd.add(["--mappings", mappings_file])
    cmd.hidden(class_to_source_files)
    actions.run(cmd, category = "merge_class_to_srcs_map")
    return output
