# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//java:java_providers.bzl", "JavaPackagingDepTSet")
load(
    "@prelude//java/plugins:java_annotation_processor.bzl",
    "JavaProcessorsInfo",
    "JavaProcessorsType",
    "derive_transitive_deps",
)

PluginParams = record(
    processors = field(["string"]),
    args = field({
        str: cmd_args,
    }),
    deps = field(["JavaPackagingDepTSet", None]),
)

def create_plugin_params(ctx: AnalysisContext, plugins: list[Dependency]) -> [PluginParams.type, None]:
    processors = []
    plugin_deps = []

    # Compiler plugin derived from `plugins` attribute
    for plugin in filter(None, [x.get(JavaProcessorsInfo) for x in plugins]):
        if plugin.type == JavaProcessorsType("plugin"):
            if len(plugin.processors) > 1:
                fail("Only 1 java compiler plugin is expected. But received: {}".format(plugin.processors))
            processors.append(plugin.processors[0])
            if plugin.deps:
                plugin_deps.append(plugin.deps)

    if not processors:
        return None

    return PluginParams(
        processors = dedupe(processors),
        deps = ctx.actions.tset(JavaPackagingDepTSet, children = plugin_deps) if plugin_deps else None,
        args = {},
    )

def java_plugin_impl(ctx: AnalysisContext) -> list["provider"]:
    if ctx.attrs._build_only_native_code:
        return [DefaultInfo()]

    return [
        JavaProcessorsInfo(
            deps = derive_transitive_deps(ctx, ctx.attrs.deps),
            processors = [ctx.attrs.plugin_name],
            type = JavaProcessorsType("plugin"),
        ),
        DefaultInfo(default_output = None),
    ]
