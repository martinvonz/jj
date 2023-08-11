# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")
load("@prelude//java:java_library.bzl", "compile_to_jar")
load("@prelude//java:java_providers.bzl", "JavaLibraryInfo", "JavaPackagingDepTSet", "JavaPackagingInfo", "create_java_packaging_dep", "derive_compiling_deps")
load(":android_providers.bzl", "AndroidBuildConfigInfo", "BuildConfigField", "merge_android_packageable_info")

def android_build_config_impl(ctx: AnalysisContext) -> list["provider"]:
    providers = []

    default_build_config_fields = get_build_config_fields(ctx.attrs.values)
    android_build_config_info = AndroidBuildConfigInfo(package = ctx.attrs.package, build_config_fields = default_build_config_fields)
    providers.append(android_build_config_info)
    providers.append(merge_android_packageable_info(ctx.label, ctx.actions, deps = [], build_config_info = android_build_config_info))

    build_config_dot_java_library, java_packaging_info = generate_android_build_config(
        ctx,
        ctx.attrs.name,
        ctx.attrs.package,
        False,
        default_build_config_fields,
        ctx.attrs.values_file,
    )

    providers.append(java_packaging_info)
    providers.append(build_config_dot_java_library)

    providers.append(DefaultInfo(default_output = build_config_dot_java_library.library_output.full_library))
    return providers

def generate_android_build_config(
        ctx: AnalysisContext,
        source: str,
        java_package: str,
        use_constant_expressions: bool,
        default_values: list["BuildConfigField"],
        values_file: ["artifact", None]) -> ("JavaLibraryInfo", "JavaPackagingInfo"):
    build_config_dot_java = _generate_build_config_dot_java(ctx, source, java_package, use_constant_expressions, default_values, values_file)

    compiled_build_config_dot_java = _compile_and_package_build_config_dot_java(ctx, java_package, build_config_dot_java)
    library_output = compiled_build_config_dot_java.classpath_entry

    packaging_deps_kwargs = {"value": create_java_packaging_dep(ctx, library_output.full_library)}
    packaging_deps = ctx.actions.tset(JavaPackagingDepTSet, **packaging_deps_kwargs)
    return (JavaLibraryInfo(
        compiling_deps = derive_compiling_deps(ctx.actions, library_output, []),
        library_output = library_output,
        output_for_classpath_macro = library_output.full_library,
    ), JavaPackagingInfo(
        packaging_deps = packaging_deps,
    ))

def _generate_build_config_dot_java(
        ctx: AnalysisContext,
        source: str,
        java_package: str,
        use_constant_expressions: bool,
        default_values: list["BuildConfigField"],
        values_file: ["artifact", None]) -> "artifact":
    generate_build_config_cmd = cmd_args(ctx.attrs._android_toolchain[AndroidToolchainInfo].generate_build_config[RunInfo])
    generate_build_config_cmd.add([
        "--source",
        source,
        "--java-package",
        java_package,
        "--use-constant-expressions",
        str(use_constant_expressions),
    ])

    default_values_file = ctx.actions.write(
        _get_output_name(java_package, "default_values"),
        ["{} {} = {}".format(x.type, x.name, x.value) for x in default_values],
    )
    generate_build_config_cmd.add(["--default-values-file", default_values_file])
    if values_file:
        generate_build_config_cmd.add(["--values-file", values_file])

    build_config_dot_java = ctx.actions.declare_output(_get_output_name(java_package, "BuildConfig.java"))
    generate_build_config_cmd.add(["--output", build_config_dot_java.as_output()])

    ctx.actions.run(
        generate_build_config_cmd,
        category = "android_generate_build_config",
        identifier = java_package,
    )

    return build_config_dot_java

def _compile_and_package_build_config_dot_java(
        ctx: AnalysisContext,
        java_package: str,
        build_config_dot_java: "artifact") -> "JavaCompileOutputs":
    return compile_to_jar(
        ctx,
        actions_identifier = "build_config_{}".format(java_package.replace(".", "_")),
        srcs = [build_config_dot_java],
    )

def get_build_config_fields(lines: list[str]) -> list["BuildConfigField"]:
    return [_get_build_config_field(line) for line in lines]

def _get_build_config_field(line: str) -> "BuildConfigField":
    type_and_name, value = [x.strip() for x in line.split("=")]
    field_type, name = type_and_name.split()
    return BuildConfigField(type = field_type, name = name, value = value)

def _get_output_name(java_package: str, output_filename: str) -> str:
    return "android_build_config/{}/{}".format(java_package.replace(".", "_"), output_filename)
