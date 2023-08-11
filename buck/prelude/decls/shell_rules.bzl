# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(":common.bzl", "buck", "prelude_rule")

sh_binary = prelude_rule(
    name = "sh_binary",
    docs = """
        An `sh_binary()` is used to execute a shell script.
    """,
    examples = """
        This sh\\_binary() just cats a sample data file back at the user.

        ```

        # $REPO/BUCK
        sh_binary(
            name = "script",
            main = "script.sh",
            resources = [
                "data.dat",
            ],
        )

        ```


        ```

        # Sample data file with data we need at runtime
        $ echo "I'm a datafile" > data.dat

        # Create a simple script that prints out the resource
        $ cat > script.sh
        #!/bin/sh
        cat $BUCK_DEFAULT_RUNTIME_RESOURCES/data.dat

        # Make sure the script is executable
        $ chmod u+x script.sh

        # Run the script, and see that it prints out the resource we provided
        $ buck run //:script
        Jobs completed: 4. Time elapsed: 0.2s.
        BUILD SUCCEEDED
        I'm a datafile

        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "main": attrs.source(doc = """
                Either the path to the script (relative to the build file), or a `build target`.
                 This file must be executable in order to be run.
            """),
            "resources": attrs.list(attrs.source(allow_directory = True), default = [], doc = """
                A list of files or build rules that this rule requires in order to run. These could be things such as
                 random data files.


                 When the script runs, the `$BUCK_DEFAULT_RUNTIME_RESOURCES`
                 environment variable specifies the directory that contains these resources.
                 This directory's location is determined entirely by Buck; the script should
                 not assume the directory's location.


                 The resources are also made available in a tree structure that mirrors
                 their locations in the source and `buck-out` trees. The
                 environment variable `$BUCK_PROJECT_ROOT` specifies a directory
                 that contains all the resources, laid out in their locations relative to
                 the original buck project root.
            """),
            "contacts": attrs.list(attrs.string(), default = []),
            "default_host_platform": attrs.option(attrs.configuration_label(), default = None),
            "deps": attrs.list(attrs.dep(), default = []),
            "labels": attrs.list(attrs.string(), default = []),
            "licenses": attrs.list(attrs.source(), default = []),
            "_target_os_type": buck.target_os_type_arg(),
        }
    ),
)

sh_test = prelude_rule(
    name = "sh_test",
    docs = """
        A `sh_test()` is a test rule that can pass results to the test runner by invoking a shell script.


        **NOTE:** This rule is not currently supported on Windows.
    """,
    examples = """
        This sh\\_test() fails if a string does not match a value.

        ```

        # $REPO/BUCK
        sh_test(
            name = "script_pass",
            test = "script.sh",
            args = ["--pass"],
        )

        sh_test(
            name = "script_fail",
            test = "script.sh",
            args = ["--fail"],
        )


        ```


        ```

        # Create a simple script that prints out the resource
        $ cat > script.sh
        #!/bin/sh
        for arg in $@; do
          if [ "$arg" == "--pass" ]; then
            echo "Passed"
            exit 0;
          fi
        done
        echo "Failed"
        exit 1

        # Make sure the script is executable
        $ chmod u+x script.sh

        # Run the script, and see that one test passes, one fails
        $ buck test //:script_pass //:script_fail
        FAILURE script.sh sh_test
        Building: finished in 0.0 sec (100%) 2/2 jobs, 0 updated
          Total time: 0.0 sec
        Testing: finished in 0.0 sec (1 PASS/1 FAIL)
        RESULTS FOR //:script_fail //:script_pass
        FAIL    <100ms  0 Passed   0 Skipped   1 Failed   //:script_fail
        FAILURE script.sh sh_test
        ====STANDARD OUT====
        Failed

        PASS    <100ms  1 Passed   0 Skipped   0 Failed   //:script_pass
        TESTS FAILED: 1 FAILURE
        Failed target: //:script_fail
        FAIL //:script_fail

        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "test": attrs.option(attrs.one_of(attrs.dep(), attrs.source()), default = None, doc = """
                Either the path to the script (relative to the build file), or a `build target`.
                 This file must be executable in order to be run.
            """),
            "args": attrs.list(attrs.arg(), default = [], doc = """
                The list of arguments to invoke this script with. These are literal values, and no shell interpolation is done.

                 These can contain `string parameter macros`
                , for example, to give the location of a generated binary to the test script.
            """),
            "env": attrs.dict(key = attrs.string(), value = attrs.arg(), sorted = False, default = {}, doc = """
                Environment variable overrides that should be used when running the script. The key is the variable name, and the value is its value.

                 The values can contain `string parameter macros`
                such as the location of a generated binary to be used by the test script.
            """),
            "type": attrs.option(attrs.string(), default = None, doc = """
                If provided, this will be sent to any configured `.buckconfig`
            """),
            "contacts": attrs.list(attrs.string(), default = []),
            "default_host_platform": attrs.option(attrs.configuration_label(), default = None),
            "deps": attrs.list(attrs.dep(), default = []),
            "labels": attrs.list(attrs.string(), default = []),
            "licenses": attrs.list(attrs.source(), default = []),
            "list_args": attrs.option(attrs.list(attrs.string()), default = None),
            "list_env": attrs.option(attrs.dict(key = attrs.string(), value = attrs.string(), sorted = False), default = None),
            "remote_execution": buck.re_opts_for_tests_arg(),
            "resources": attrs.list(attrs.source(), default = []),
            "run_args": attrs.list(attrs.string(), default = []),
            "run_env": attrs.dict(key = attrs.string(), value = attrs.string(), sorted = False, default = {}),
            "run_test_separately": attrs.bool(default = False),
            "test_rule_timeout_ms": attrs.option(attrs.int(), default = None),
        }
    ),
)

shell_rules = struct(
    sh_binary = sh_binary,
    sh_test = sh_test,
)
