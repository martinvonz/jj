# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:paths.bzl", "paths")
load(
    "@prelude//tests:re_utils.bzl",
    "get_re_executor_from_props",
)
load("@prelude//utils:utils.bzl", "from_named_set", "value_or")
load("@prelude//test/inject_test_run_info.bzl", "inject_test_run_info")
load(":make_pex.bzl", "PexProviders", "make_default_info")
load(":python_binary.bzl", "python_executable")
load(":python_library.bzl", "py_attr_resources", "qualify_srcs")

def _write_test_modules_list(
        ctx: AnalysisContext,
        srcs: dict[str, "artifact"]) -> (str, "artifact"):
    """
    Generate a python source file with a list of all test modules.
    """
    name = "__test_modules__.py"
    contents = "TEST_MODULES = [\n"
    for dst in srcs:
        root, ext = paths.split_extension(dst)
        if ext != ".py":
            fail("test sources must end with .py")
        module = root.replace("/", ".")
        contents += "    \"{}\",\n".format(module)
    contents += "]\n"
    return name, ctx.actions.write(name, contents)

def python_test_executable(ctx: AnalysisContext) -> PexProviders.type:
    main_module = value_or(ctx.attrs.main_module, "__test_main__")

    srcs = qualify_srcs(ctx.label, ctx.attrs.base_module, from_named_set(ctx.attrs.srcs))

    # Generate the test modules file and add it to sources.
    test_modules_name, test_modules_path = _write_test_modules_list(ctx, srcs)
    srcs[test_modules_name] = test_modules_path

    # Add in default test runner.
    srcs["__test_main__.py"] = ctx.attrs._test_main

    resources = qualify_srcs(ctx.label, ctx.attrs.base_module, py_attr_resources(ctx))

    return python_executable(
        ctx,
        main_module,
        srcs,
        resources,
        compile = value_or(ctx.attrs.compile, False),
    )

def python_test_impl(ctx: AnalysisContext) -> list["provider"]:
    pex = python_test_executable(ctx)
    test_cmd = pex.run_cmd

    # Setup a RE executor based on the `remote_execution` param.
    re_executor = get_re_executor_from_props(ctx.attrs.remote_execution)

    return inject_test_run_info(
        ctx,
        ExternalRunnerTestInfo(
            type = "pyunit",
            command = [test_cmd],
            env = ctx.attrs.env,
            labels = ctx.attrs.labels,
            contacts = ctx.attrs.contacts,
            default_executor = re_executor,
            # We implicitly make this test via the project root, instead of
            # the cell root (e.g. fbcode root).
            run_from_project_root = re_executor != None,
            use_project_relative_paths = re_executor != None,
        ),
    ) + [make_default_info(pex)]
