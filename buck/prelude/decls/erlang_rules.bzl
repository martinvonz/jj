# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//erlang/erlang_application.bzl", "StartTypeValues")
load(":common.bzl", "prelude_rule")

common_attributes = {
    "contacts": attrs.list(attrs.string(), default = []),
    "labels": attrs.list(attrs.string(), default = []),
    "os_env": attrs.option(attrs.dict(key = attrs.string(), value = attrs.string()), default = None, doc = """
                This attribute allows to set additional values for the operating system environment for invocations to the
                Erlang toolchain.
            """),
}

common_shell_attributes = (
    {
        "shell_configs": attrs.set(attrs.dep(), default = read_root_config("erlang", "shell_configs", "").split(), doc = """
            This attribute allows to set config files for the shell. The dependencies that are typically used
            here are `export_file` targets.
        """),
        "shell_libs": attrs.set(
            attrs.dep(),
            default = ["prelude//erlang/shell:buck2_shell_utils"],
            doc = """
            This attribute allows to define additional dependencies for the shell. By default this is
            set to `["prelude//erlang/shell:buck2_shell_utils"]` which includes a `user_default` module
            that loads and compiles modules with buck2 mechanisms.
        """,
        ),
    }
)

common_application_attributes = dict({
    "applications": attrs.list(attrs.dep(), default = [], doc = """
        Equivalent to the corresponding `applications` and `included_applications`
        fields you will find in `*.app.src` or `*.app` files and specify the application dependencies. Contrary to the
        fields in the `*.app.src` or `*.app` files, **it is necessary to use target paths to the application** where a
        dependency is desired. These fields will be used to construct equally named fields in the generated `*.app` file
        for the application.

        OTP applications are specified with the target path `otp//:<application>`.

        **NOTE**: _If you use the `app_src` field and the references application resource file template specifies
        `applications` or `included_applications` buck2 checks that the target definitions and information in the template are
        equivalent to prevent these definitions from drifting apart during migration._
    """),
    "included_applications": attrs.list(attrs.dep(), default = [], doc = """
        Check the documentation for `applications`.
    """),
    "version": attrs.string(default = "1.0.0", doc = """
        The `version` field specifies the applications version that is materialized as `vsn` field in the generated `*.app`
        file. If you use the the `app_src` field and specify a version in the referenced template in addition to the version
        field, the versions need to be identical.

        If no version is specified in either the `app_src` template or the `version` field, a fallback version string of
        `"1.0.0"` is used.
    """),
    "_toolchain": attrs.toolchain_dep(default = "toolchains//:erlang-default"),
}, **common_shell_attributes)

rules_attributes = {
    "erlang_app": dict(
        {
            "app_src": attrs.option(attrs.source(), default = None, doc = """
                The `app_src` field allows to optionally reference a `*.app.src` template file. This template file will then be used by
                buck2 to generate the `*.app` output file in the applications `ebin/` directory. This is useful during the migration from
                rebar3 to buck2 to avoid duplicated entries, of e.g. the `version`.

                Buck2 will use or check all fields present in the template, and fill out the fields with the information provided in the
                target, e.g. if the `version` is specified in both, buck2 will check that they are identical. Otherwise, it uses the
                information from the template if the target doesn't specify it, and vice versa.

                **NOTE**: _If you use the `app_src` field and the references application resource file template specifies `applications`
                or `included_applications` buck2 checks that the target definitions and information in the template are equivalent to
                prevent these definitions from drifting apart during migration._
            """),
            "build_edoc_chunks": attrs.bool(default = True, doc = """
                This attribute controls if the output of the builds also create edoc chunks.
            """),
            "env": attrs.option(attrs.dict(key = attrs.string(), value = attrs.string()), default = None, doc = """
                The `env` field allows to set the application env variables. The key value pairs will materialise in tha applications `.app`
                file and can then be accessed by [`application:get_env/2`](https://www.erlang.org/doc/man/application.html#get_env-2).
           """),
            "erl_opts": attrs.option(attrs.list(attrs.string()), default = None, doc = """
                Typically compile options are managed by global config files, however, sometimes it is
                desirable to overwrite the pre-defined compile options. The `erl_opts` field allows developers to do so for individual
                applications.

                The main use-case are the applications listed in `third-party/`. This option should not be used by other applications
                without consultation. Please ask in the [WhatsApp Dev Infra Q&A](https://fb.workplace.com/groups/728545201114362)
                workplace group for support.
            """),
            "extra_includes": attrs.list(attrs.dep(), default = [], doc = """
                In some cases we might have the situation, where an application `app_a` depends through the `applications` and
                `included_applications` fields on application `app_b` and a source file in `app_b` includes a header file from `app_a`
                (e.g. `-include_lib("app_a/include/header.hrl`). This technically creates circular dependency from `app_a` to `app_b`
                (e.g. via `applications` field) and back from `app_b` to `app_a` (via `-include_lib`). To break the dependency
                developers can specify targets in the `extra_includes` field, whose public include files are accessible to the
                application target during build time.

                Only the includes of the specified application are available and eventual transitive dependencies need to be managed
                manually.

                **NOTE**: _It is not possible (or even desired) to add OTP applications with this field._

                **NOTE**: _This mechanism is added to circumvent unclean dependency relationships and the goal for
                developers should be to reduce usages of this field._ **DO NOT ADD ANY MORE USAGES!!**
            """),
            "extra_properties": attrs.option(attrs.dict(key = attrs.string(), value = attrs.one_of(attrs.string(), attrs.list(attrs.string()))), default = None, doc = """
                The extra_properties field can be used to specify extra key-value pairs which is are not defined in
                [application_opt()](https://www.erlang.org/doc/man/application.html#load-2). The key-value pair will be stored in the
                applications `.app` file and can be accessed by `file:consult/1`.
            """),
            "includes": attrs.list(attrs.source(), default = [], doc = """
                The public header files accessible via `-include_lib("appname/include/header.hrl")` from other erlang files.
            """),
            "mod": attrs.option(attrs.tuple(attrs.string(), attrs.list(attrs.string())), default = None, doc = """
                The `mod` field specifies the equivalent field in the generated `*.app` files. The format is similar, with the
                difference, that the module name, and the individual start arguments need to be given as the string representation
                of the corresponding Erlang terms.
             """),
            "resources": attrs.list(attrs.dep(), default = [], doc = """
                The `resources` field specifies targets whose default output are placed in the applications `priv/` directory. For
                regular files this field is typically combined with `export_file`, `filegroup`, or similar targets. However, it
                is general, and any target can be used, e.g. if you want to place a built escript in the `priv/` directory, you can use
                an `erlang_escript` target.
            """),
            "srcs": attrs.list(attrs.source(), default = [], doc = """
                A list of `*.erl`, `*.hrl`, `*.xrl`, or `*.yrl` source inputs that are typically located
                in an application's `src/` folder. Header files (i.e. `*.hrl` files) specified in this field are considered application
                private headers, and can only be accessed by the `*.erl` files of the application itself. `*.xrl` and `*.yrl` files are
                processed into `*.erl` files before all `*.erl` files are compiled into `*.beam` files.
            """),
            "use_global_parse_transforms": attrs.bool(default = True, doc = """
                This field indicates if global parse_tranforms should be applied to this application as well. It often makes sense
                for third-party dependencies to not be subjected to global parse_transforms, similar to OTP applications.
            """),
        },
        **common_application_attributes
    ),
    "erlang_app_includes": {
        "application_name": attrs.string(),
        "includes": attrs.list(attrs.source(), default = []),
        "_toolchain": attrs.toolchain_dep(default = "toolchains//:erlang-default"),
    },
    "erlang_escript": {
        "deps": attrs.list(attrs.dep(), doc = """
                List of Erlang applications that are bundled in the escript. This includes all transitive dependencies as well.
            """),
        "emu_args": attrs.list(attrs.string(), default = [], doc = """
                This field specifies the emulator flags that the escript uses on execution. It is often desirable to specify the number
                of threads and schedulers the escript uses. Please refer to the
                [OTP documentation](https://www.erlang.org/doc/man/erl.html#emu_flags) for details.
            """),
        "include_priv": attrs.bool(default = False, doc = """
                Setting this flag, will package the applications `priv` directory in the escript. Similar to files added through the
                `resources` field, the `priv` folders files can then be accessed by `escript"extract/2`.
            """),
        "main_module": attrs.option(attrs.string(), default = None, doc = """
                Overrides the default main module. Instead of defering the main module from the scripts filename, the specified module
                is used. That module needs to export a `main/1` function that is called as entry point.
            """),
        "resources": attrs.list(attrs.dep(), default = [], doc = """
                This adds the targets default output to the escript archive. To access these files, you need to use `escript:extract/2`,
                which will extract the entire escript in memory. The relevant files can then be accessed through the `archive`
                section.

                Please refer to the [`escript:extract/2`](https://www.erlang.org/doc/man/escript.html) for more details.
            """),
        "script_name": attrs.option(attrs.string(), default = None, doc = """
                Overrides the filename of the produced escript.
            """),
        "_toolchain": attrs.toolchain_dep(default = "toolchains//:erlang-default"),
    },
    "erlang_otp_binaries": {
        "erl": attrs.source(doc = """
                Reference to `erl` binary
            """),
        "erlc": attrs.source(doc = """
                Reference to `erlc` binary
            """),
        "escript": attrs.source(doc = """
                Reference to `escript` binary
            """),
    },
    "erlang_release": {
        "applications": attrs.list(attrs.one_of(attrs.dep(), attrs.tuple(attrs.dep(), attrs.enum(StartTypeValues))), doc = """
                This field specifies the list of applications that the release should start in the given order, and optionally the start
                type. Top-level applications without given start type are started with type
                [`permanent`](https://www.erlang.org/doc/man/application.html#type-restart_type).
            """),
        "include_erts": attrs.bool(default = False, doc = """
                This field controls wether OTP applications and the Erlang runtime system should be included as part of the release.
                Please note, that at the moment the erts folder is just `erts/`.
            """),
        "multi_toolchain": attrs.option(attrs.list(attrs.dep()), default = None, doc = """
                This field controls wether the release should be built with a single toolchain, or multiple toolchains. In the
                latter case, all output paths are prefixed with the toolchain name.
            """),
        "overlays": attrs.dict(key = attrs.string(), value = attrs.list(attrs.dep()), default = {}, doc = """
                Overlays can be used to add files to the release. They are specified as mapping from path (from the release
                root) to list of targets. The targets files are places **flat** at the target location with their basename.
            """),
        "release_name": attrs.option(attrs.string(), default = None, doc = """
                The release name can explicitly be set by this field. This overwrites the default from the target name.
            """),
        "version": attrs.string(default = "1.0.0", doc = """
                The `version` field specifies the release version. The release version is used in the release resource file, and
                is part of the path for the folder containing the boot scripts.
            """),
        "_toolchain": attrs.toolchain_dep(default = "toolchains//:erlang-default"),
    },
    "erlang_test": dict(
        {
            "config_files": attrs.list(attrs.dep(), default = [], doc = """
                Will specify what config files the erlang beam machine running test with should load, for reference look at
                [OTP documentation](https://www.erlang.org/doc/man/config.html). These ones should consist of default_output of
                some targets. In general, this field is filled with target coming from then `export_file` rule, as in the example below.
            """),
            "deps": attrs.list(attrs.dep(), default = [], doc = """
                The set of dependencies needed for all suites included in the target
                to compile and run. They could be either `erlang_app(lication)` or `erlang_test`
                targets, although the latter is discouraged. If some suites need to access common methods,
                a common helper file should be created and included in the `srcs` field of the `erlang_tests` target.
                If some applications are included as dependencies of this target, their private include will automatically
                be pulled and made available for the test. That allows tests to access the private header files from the
                applications under test.
            """),
            "env": attrs.dict(key = attrs.string(), value = attrs.string(), default = {}, doc = """
                Add the given values to the environment variables with which the test is executed.
            """),
            "extra_ct_hooks": attrs.list(attrs.string(), default = [], doc = """
                List of additional Common Test hooks. The strings are interpreted as Erlang terms.
            """),
            "preamble": attrs.string(default = read_root_config("erlang", "erlang_test_preamble", "test:info(),test:ensure_initialized(),user_drv:start()."), doc = """
            """),
            "property_tests": attrs.list(attrs.dep(), default = [], doc = """
            """),
            "resources": attrs.list(attrs.dep(), default = [], doc = """
                The `resources` field specifies targets whose default output are placed in the test `data_dir` directory for
                all the suites present in the macro target. Additionally, if data directory are present in the directory along
                the suite, this one will be pulled automatically for the relevant suite.

                Any target can be used, e.g. if you want to place a built escript in the `data_dir` directory, you can use
                an `erlang_escript` target.
            """),
            "suite": attrs.source(doc = """
                The source file for the test suite. If you are using the macro, you should use the `suites` attribute instead.

                The suites attribtue specify which erlang_test targets should be generated. For each suite "path_to_suite/suite_SUITE.erl" an
                implicit 'erlang_test' target suite_SUITE will be generated.
            """),
            "_cli_lib": attrs.dep(default = "prelude//erlang/common_test/test_cli_lib:test_cli_lib"),
            "_ct_opts": attrs.string(default = read_root_config("erlang", "erlang_test_ct_opts", "")),
            "_providers": attrs.string(),
            "_test_binary": attrs.dep(default = "prelude//erlang/common_test/test_binary:escript"),
            "_test_binary_lib": attrs.dep(default = "prelude//erlang/common_test/test_binary:test_binary"),
            "_toolchain": attrs.toolchain_dep(default = "toolchains//:erlang-default"),
            "_trampoline": attrs.option(attrs.dep(), default = None),
        },
        **common_shell_attributes
    ),
}

attributes = {
    name: dict(rules_attributes[name], **common_attributes)
    for name in rules_attributes
}

erlang_app = prelude_rule(
    name = "erlang_app",
    docs = """
        This rule is the main rule for Erlang applications. It gets generated by using the `erlang_application`
        macro, that takes as attributes the same attributes as this rule. You should always use the
        `erlang_application` macro instead of using this rule directly.

        Erlang Applications are the basic building block of our buck2 integration and used by many other Erlang
        targets, e.g. `erlang_escript`, `erlang_test`, or `erlang_release`.

        The `erlang_application` targets build OTP applications and as such many attributes that are used have
        equivalent meaning to the fields in the currently (by rebar3) used `*.app.src` files and OTP `*.app`
        files. Please familiarize yourself with the semantics of these fields by consulting the
        [OTP documentation](https://erlang.org/doc/man/app.html).

        The target enforces uniqueness during builds, and fails to build if duplicated artifacts in the
        global namespaces are detected:
        - duplicated application names in the dependencies
        - duplicated module names across any of the applications or dependencies modules
        - ambiguity when resolving header files

        The default output of this rule is the application folder of the target application and all transitive dependencies.
    """,
    examples = """
        #### Minimal Erlang Application

        ```
        erlang_application(
            name = "minimal",
        )
        ```

        #### With `priv/` directory

        ```
        erlang_application(
            name = "app_a",
            srcs = [
                "src/app_a.erl",
            ],
            includes = [],
            applications = [
                ":app_b",
            ],
            app_src = "src/app_a.app.src",
            resources = [
                ":readme",
            ],
        )

        export_file(
            name = "readme",
            src = "README.md",
        )
        ```

        #### Using OTP applications and `mod` field

        ```
        erlang_application(
            name = "app_b",
            srcs = [
                "src/app_b.erl",
                "src/app_b.hrl",
            ],
            includes = [],
            applications = [
                "kernel",
                "stdlib",
                ":app_c",
            ],
            mod = ("app_b", [
                "some_atom",
                "\"some string\"",
                "{tagged_tuple, 42}",
            ]),
        )
        ```

        #### Using Yecc and Leex

        ```
        erlang_application(
            name = "yecc_leex",
            srcs = [
                "src/leex_stub.xrl",
                "src/yecc_stub.yrl",
            ],
        )
        ```
    """,
    further = None,
    attrs = attributes["erlang_app"],
)

erlang_app_includes = prelude_rule(
    name = "erlang_app_includes",
    docs = """
        This rule is a supplementary rule for Erlang applications. It gets generated by using the `erlang_application`
        macro, that takes as attributes the same attributes as this rule. You should always use the
        `erlang_application` macro instead of using this rule directly.
    """,
    further = None,
    attrs = attributes["erlang_app_includes"],
)

erlang_escript = prelude_rule(
    name = "erlang_escript",
    docs = """
        The `erlang_escript` target builds and runs bundled escripts. Please refer to the
        [OTP documentation](https://www.erlang.org/doc/man/escript.html) for more details about escripts.

        Escripts by default always try to use the module that has the same name as the escripts basename as entry point, e.g. if
        the escript is called `script.escript` then running the escript will try to call `script:main/1`. Both name and
        main module can be overwritten though.

        The target name doubles as the default escript name. If the `main_module` attribute is not used, the escript filename will
        be `<name>.escript`.
    """,
    examples = """
        ```
        erlang_escript(
            name = "script",
            main_module = "main_module",
            script_name = "the_script",
            deps = [
                ":escript_app",
            ],
            emu_args = ["+sbtu", "+A1"],
        )

        erlang_application(
            name = "escript_app",
            srcs = ["src/main_module.erl"],
            applications = [
                "kernel",
                "stdlib",
            ],
        )
        ```
    """,
    further = None,
    attrs = attributes["erlang_escript"],
)

erlang_otp_binaries = prelude_rule(
    name = "erlang_otp_binaries",
    docs = """
        This target defines the executables for the Erlang toolchains, and is required to defined a toolchain.
    """,
    examples = """
        erlang_otp_binaries(
            name = "local",
            erl = "local/erl",
            erlc = "local/erlc",
            escript = "local/escript",
        )
    """,
    further = None,
    attrs = attributes["erlang_otp_binaries"],
)

erlang_release = prelude_rule(
    name = "erlang_release",
    docs = """
        The `erlang_release` target builds OTP releases. Please refer to the
        [OTP documentation](https://www.erlang.org/doc/design_principles/release_structure.html) for more details about
        releases.

        The `erlang_release` target does by default (without overlays) package:
        - applications that are required to start the release
        - release resource file `<relname>.rel` (see [rel(4)](https://www.erlang.org/doc/man/rel.html))
        - boot script `start.script` (see [rel(4)](https://www.erlang.org/doc/man/script.html))
        - binary boot script `start.boot`
        - `bin/release_variables`

        The `release_variables` file contains release name, version, and erts version in shell syntax, e.g.
        ```
        ERTS_VSN="12.1.2"
        REL_NAME="rel1"
        REL_VSN="1.0.0"
        ```

        The target name doubles as the default release name. If the `release_name` attribute is used, the release name will be
        sources from there instead.
    """,
    examples = """
        ```
        erlang_release(
            name = "world",
            version = "1.0.0",
            applications = [
                "//apps//app_a:app_a",
                "//apps//app_b:app_b",
            ],
            overlays = {
                "releases/1.0.0": [
                    ":sys.config.src",
                ],
                "bin": [
                    ":start.sh",
                ],
            },
        )

        export_file(
            name = "sys.config.src",
            src = "sys.config",
        )

        export_file(
            name = "start.sh",
            src = "start.sh",
        )
        ```
    """,
    further = None,
    attrs = attributes["erlang_release"],
)

erlang_test = prelude_rule(
    name = "erlang_test",
    docs = """
        The `erlang_test` ruls defines a test target for a single test suite. In most cases you
        want to define multiple suites in one go. The `erlang_tests` macro allows users to generate
        `erlang_test` targets for multiple test suites. Each suite `<name>_SUITE.erl` will have a
        generated hidden `erlang_test` target whose name is `<name>_SUITE`.

        Each `erlang_test` target implements tests using the Common Test library
        [OTP documentation](https://www.erlang.org/doc/man/common_test.html). They can,
        although **it is not recommended**, also act as dependencies of other tests. The
        default output of this rule is a "test_folder", consisting of the compiled test suite
        and the data directory.


        For each suite  `<name>_SUITE.erl`, if a data_dir `<name>_SUITE_data` is present along the suite,
        (as per [the data_dir naming scheme for ct](https://www.erlang.org/doc/apps/common_test/write_test_chapter.html#data-and-private-directories)),
        it will automatically adds the coresponding resource target to the generated test target of the suite.
        Resources will be placed in the [Data directory (data_dir)](https://www.erlang.org/doc/apps/common_test/write_test_chapter.html#data_priv_dir)
        of each of the suite.

        It allows the writer of the rule to add global configuration files and global default
        dependencies (e.g `meck`). These ones should be specified using global
        variables `erlang.erlang_tests_default_apps` and `erlang.erlang_tests_default_config`
        respectively.

        The `erlang_tests` macro forwards all attributes to the `erlang_test`. It defines some attributes
        that control how the targets get generated:
        - `use_default_configs` (bool): Parameter that controls if the config files specified by the global config variable
          `erlang.erlang_tests_default_config` should be used, default to True.
        - `use_default_deps` (bool): Parameter that controls if the dependencies specified by the global config variable
          `erlang.erlang_tests_default_apps` should be pulled, default to True.
        - `srcs` ([source]): Set of files that the suites might depend on and that are not part of any specific application.
          A "meta" application having those files as sources will automatically be created, and included in the dependencies
          of the tests.

        Ene can call
        - `buck2 build //my_app:test_SUITE` to compile the test files together with its depedencies.
        - `buck2 test //my_app:other_test_SUITE` to run the test.
        - `buck2 run //my_app:other_test_SUITE` to open an interactive test shell, where tests can be run iteratively.


        buck2 test will rely on tpx to run the suite. To get access to tpx commands, add `--` after the
        target. For example:

        - `buck2 test //my_app:other_test_SUITE -- --help` will print the list of tpx available
        command line parameters.
        - `buck2 test //my_app:other_test_SUITE -- group.mycase` will only run those test cases
        that match the pattern `group.mycase`
    """,
    examples = """
        erlang_test(
            name = "unit_test_SUITE",
            suite = "unit_test_SUTIE.erl",
            deps = [":my_other_app"],
            contacts = ["author@email.com"],
        )

        erlang_tests(
            suites = ["test_SUITE.erl", "other_test_SUITE".erl],
            deps = [":my_app"],
            contacts = ["author@email.com"],
        )
    """,
    further = None,
    attrs = attributes["erlang_test"],
)

erlang_rules = struct(
    erlang_app = erlang_app,
    erlang_app_includes = erlang_app_includes,
    erlang_escript = erlang_escript,
    erlang_otp_binaries = erlang_otp_binaries,
    erlang_release = erlang_release,
    erlang_test = erlang_test,
)
