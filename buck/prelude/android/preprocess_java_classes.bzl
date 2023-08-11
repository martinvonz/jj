# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")
load("@prelude//java:java_toolchain.bzl", "JavaToolchainInfo")
load("@prelude//java/utils:java_utils.bzl", "get_path_separator")
load("@prelude//utils:utils.bzl", "expect")

def get_preprocessed_java_classes(ctx: AnalysisContext, input_jars = {"artifact": "target_label"}) -> dict["artifact", "target_label"]:
    if not input_jars:
        return {}

    sh_script, _ = ctx.actions.write(
        "preprocessed_java_classes/script.sh",
        cmd_args(ctx.attrs.preprocess_java_classes_bash),
        is_executable = True,
        allow_args = True,
    )

    preprocess_cmd = cmd_args(["/usr/bin/env", "bash", sh_script])
    preprocess_cmd.hidden(cmd_args(ctx.attrs.preprocess_java_classes_bash))
    for dep in ctx.attrs.preprocess_java_classes_deps:
        preprocess_cmd.hidden(dep[DefaultInfo].default_outputs + dep[DefaultInfo].other_outputs)

    input_srcs = {}
    unscrubbed_output_jars_to_owners = {}
    unscrubbed_output_dir = ctx.actions.declare_output("preprocessed_java_classes/unscrubbed_output_dir")
    zip_scrubber = ctx.attrs._java_toolchain[JavaToolchainInfo].zip_scrubber

    for i, (input_jar, target_label) in enumerate(input_jars.items()):
        expect(input_jar.extension == ".jar", "Expected {} to have extension .jar!".format(input_jar))
        jar_name = "{}_{}".format(i, input_jar.basename)
        input_srcs[jar_name] = input_jar
        unscrubbed_output_jar = unscrubbed_output_dir.project(jar_name)
        preprocess_cmd.hidden(unscrubbed_output_jar.as_output())
        unscrubbed_output_jars_to_owners[unscrubbed_output_jar] = target_label

    input_dir = ctx.actions.symlinked_dir("preprocessed_java_classes/input_dir", input_srcs)

    env = {
        "ANDROID_BOOTCLASSPATH": cmd_args(
            ctx.attrs._android_toolchain[AndroidToolchainInfo].android_bootclasspath,
            delimiter = get_path_separator(),
        ),
        "IN_JARS_DIR": cmd_args(input_dir),
        "OUT_JARS_DIR": unscrubbed_output_dir.as_output(),
    }

    ctx.actions.run(preprocess_cmd, env = env, category = "preprocess_java_classes")

    output_jars_to_owners = {}
    for unscrubbed_output_jar, target_label in unscrubbed_output_jars_to_owners.items():
        jar_name = unscrubbed_output_jar.basename
        output_jar = ctx.actions.declare_output("preprocessed_java_classes/output_dir/{}".format(jar_name))
        scrub_cmd = cmd_args(zip_scrubber, unscrubbed_output_jar, output_jar.as_output())
        ctx.actions.run(scrub_cmd, category = "scrub_preprocessed_java_class", identifier = jar_name)
        output_jars_to_owners[output_jar] = target_label

    return output_jars_to_owners
