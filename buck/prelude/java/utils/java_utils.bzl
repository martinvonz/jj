# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(
    "@prelude//java:class_to_srcs.bzl",
    "JavaClassToSourceMapInfo",  # @unused Used as a type
    "create_class_to_source_map_from_jar",
    "create_class_to_source_map_info",
)
load("@prelude//java:java_toolchain.bzl", "AbiGenerationMode", "JavaToolchainInfo")
load("@prelude//utils:utils.bzl", "expect")

def get_path_separator() -> str:
    # TODO: msemko : replace with system-dependent path-separator character
    # On UNIX systems, this character is ':'; on Microsoft Windows systems it is ';'.
    return ":"

def derive_javac(javac_attribute: [str, Dependency, "artifact"]) -> [str, "RunInfo", "artifact"]:
    javac_attr_type = type(javac_attribute)
    if javac_attr_type == "dependency":
        javac_run_info = javac_attribute.get(RunInfo)
        if javac_run_info:
            return javac_run_info
        outputs = javac_attribute[DefaultInfo].default_outputs
        expect(len(outputs) == 1, "Expect one default output from build dep of attr javac!")
        return outputs[0]

    if javac_attr_type == "artifact":
        return javac_attribute

    if javac_attr_type == type(""):
        return javac_attribute

    fail("Type of attribute javac {} that equals to {} is not supported.\n Supported types are \"dependency\", \"artifact\" and \"string\".".format(javac_attr_type, javac_attribute))

def get_java_version_attributes(ctx: AnalysisContext) -> (int, int):
    java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo]
    java_version = ctx.attrs.java_version
    java_source = ctx.attrs.source
    java_target = ctx.attrs.target

    if java_version:
        if java_source or java_target:
            fail("No need to set 'source' and/or 'target' attributes when 'java_version' is present")
        java_version = to_java_version(java_version)
        return (java_version, java_version)

    source = java_source or java_toolchain.source_level
    target = java_target or java_toolchain.target_level

    expect(bool(source) and bool(target), "Java source level and target level must be set!")

    source = to_java_version(source)
    target = to_java_version(target)

    expect(source <= target, "java library source level {} is higher than target {} ", source, target)

    return (source, target)

def to_java_version(java_version: str) -> int:
    if java_version.startswith("1."):
        expect(len(java_version) == 3, "Supported java version number format is 1.X, where X is a single digit number, but it was set to {}", java_version)
        java_version_number = int(java_version[2:])
        expect(java_version_number < 9, "Supported java version number format is 1.X, where X is a single digit number that is less than 9, but it was set to {}", java_version)
        return java_version_number
    else:
        return int(java_version)

def get_abi_generation_mode(abi_generation_mode):
    return {
        None: None,
        "class": AbiGenerationMode("class"),
        "migrating_to_source_only": AbiGenerationMode("source"),
        "none": AbiGenerationMode("none"),
        "source": AbiGenerationMode("source"),
        "source_only": AbiGenerationMode("source_only"),
    }[abi_generation_mode]

def get_default_info(
        actions: "actions",
        java_toolchain: "JavaToolchainInfo",
        outputs: ["JavaCompileOutputs", None],
        packaging_info: "JavaPackagingInfo",
        extra_sub_targets: dict = {}) -> DefaultInfo.type:
    sub_targets = get_classpath_subtarget(actions, packaging_info)
    default_info = DefaultInfo()
    if outputs:
        abis = [
            ("class-abi", outputs.class_abi),
            ("source-abi", outputs.source_abi),
            ("source-only-abi", outputs.source_only_abi),
        ]
        for (name, artifact) in abis:
            if artifact != None:
                sub_targets[name] = [DefaultInfo(default_output = artifact)]
        other_outputs = []
        if not java_toolchain.is_bootstrap_toolchain and outputs.annotation_processor_output:
            other_outputs.append(outputs.annotation_processor_output)
        default_info = DefaultInfo(
            default_output = outputs.full_library,
            sub_targets = extra_sub_targets | sub_targets,
            other_outputs = other_outputs,
        )
    return default_info

def declare_prefixed_name(name: str, prefix: [str, None]) -> str:
    if not prefix:
        return name

    return "{}_{}".format(prefix, name)

def get_class_to_source_map_info(
        ctx: AnalysisContext,
        outputs: ["JavaCompileOutputs", None],
        deps: list[Dependency]) -> (JavaClassToSourceMapInfo.type, dict):
    sub_targets = {}
    class_to_srcs = None
    if not ctx.attrs._is_building_android_binary and outputs != None:
        class_to_srcs = create_class_to_source_map_from_jar(
            actions = ctx.actions,
            java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo],
            name = ctx.attrs.name + ".class_to_srcs.json",
            jar = outputs.classpath_entry.full_library,
            srcs = ctx.attrs.srcs,
        )
        sub_targets["class-to-srcs"] = [DefaultInfo(default_output = class_to_srcs)]
    class_to_src_map_info = create_class_to_source_map_info(
        ctx = ctx,
        mapping = class_to_srcs,
        deps = deps,
    )
    return (class_to_src_map_info, sub_targets)

def get_classpath_subtarget(actions: "actions", packaging_info: "JavaPackagingInfo") -> dict[str, list["provider"]]:
    proj = packaging_info.packaging_deps.project_as_args("full_jar_args")
    output = actions.write("classpath", proj)
    return {"classpath": [DefaultInfo(output, other_outputs = [proj])]}
