# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//android:android_providers.bzl", "AndroidApkInfo", "AndroidInstrumentationApkInfo")
load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")
load("@prelude//java:java_toolchain.bzl", "JavaToolchainInfo")
load("@prelude//java/utils:java_utils.bzl", "get_path_separator")
load("@prelude//utils:utils.bzl", "expect")
load("@prelude//test/inject_test_run_info.bzl", "inject_test_run_info")

DEFAULT_ANDROID_SUBPLATFORM = "android-30"

def android_instrumentation_test_impl(ctx: AnalysisContext):
    android_toolchain = ctx.attrs._android_toolchain[AndroidToolchainInfo]

    cmd = [ctx.attrs._java_toolchain[JavaToolchainInfo].java_for_tests]

    classpath = android_toolchain.instrumentation_test_runner_classpath

    classpath_args = cmd_args()
    classpath_args.add("-classpath")
    classpath_args.add(cmd_args(classpath, delimiter = get_path_separator()))
    classpath_args_file = ctx.actions.write("classpath_args_file", classpath_args)
    cmd.append(cmd_args(classpath_args_file, format = "@{}").hidden(classpath_args))

    cmd.append(android_toolchain.instrumentation_test_runner_main_class)

    apk_info = ctx.attrs.apk.get(AndroidApkInfo)
    expect(apk_info != None, "Provided APK must have AndroidApkInfo!")

    instrumentation_apk_info = ctx.attrs.apk.get(AndroidInstrumentationApkInfo)
    if instrumentation_apk_info != None:
        cmd.extend(["--apk-under-test-path", instrumentation_apk_info.apk_under_test])

    target_package_file = ctx.actions.declare_output("target_package_file")
    package_file = ctx.actions.declare_output("package_file")
    test_runner_file = ctx.actions.declare_output("test_runner_file")
    manifest_utils_cmd = cmd_args(ctx.attrs._android_toolchain[AndroidToolchainInfo].manifest_utils[RunInfo])
    manifest_utils_cmd.add([
        "--manifest-path",
        apk_info.manifest,
        "--package-output",
        package_file.as_output(),
        "--target-package-output",
        target_package_file.as_output(),
        "--instrumentation-test-runner-output",
        test_runner_file.as_output(),
    ])
    ctx.actions.run(manifest_utils_cmd, category = "get_manifest_info")

    cmd.extend(
        [
            "--test-package-name",
            cmd_args(package_file, format = "@{}"),
            "--target-package-name",
            cmd_args(target_package_file, format = "@{}"),
            "--test-runner",
            cmd_args(test_runner_file, format = "@{}"),
        ],
    )

    cmd.extend(
        [
            "--adb-executable-path",
            "required_but_unused",
            "--instrumentation-apk-path",
            apk_info.apk,
        ],
    )

    test_info = ExternalRunnerTestInfo(
        type = "android_instrumentation",
        command = cmd,
        env = ctx.attrs.env,
        labels = ctx.attrs.labels,
        contacts = ctx.attrs.contacts,
        run_from_project_root = True,
        use_project_relative_paths = True,
        executor_overrides = {
            "android-emulator": CommandExecutorConfig(
                local_enabled = android_toolchain.instrumentation_test_can_run_locally,
                remote_enabled = True,
                remote_execution_properties = {
                    "platform": "android-emulator",
                    "subplatform": _compute_emulator_target(ctx.attrs.labels or []),
                },
                remote_execution_use_case = "instrumentation-tests",
            ),
            "static-listing": CommandExecutorConfig(
                local_enabled = True,
                remote_enabled = True,
                remote_execution_properties = {
                    "platform": "linux-remote-execution",
                },
                remote_execution_use_case = "buck2-default",
            ),
        },
        local_resources = {
            "android_emulator": None,
        },
    )
    return inject_test_run_info(ctx, test_info) + [
        DefaultInfo(),
    ]

# replicating the logic in https://fburl.com/code/1fqowxu4 to match buck1's behavior
def _compute_emulator_target(labels: [str]) -> str:
    emulator_target_labels = [label for label in labels if label.startswith("re_emulator_")]
    expect(len(emulator_target_labels) <= 1, "multiple 're_emulator_' labels were found:[{}], there must be only one!".format(", ".join(emulator_target_labels)))
    if len(emulator_target_labels) == 0:
        return DEFAULT_ANDROID_SUBPLATFORM
    else:  # len(emulator_target_labels) == 1:
        return emulator_target_labels[0].replace("re_emulator_", "")
