# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# This file contains the specification for all the providers the Erlang
# integration uses.

# Information about an Erlang application and its dependencies.

ErlangAppCommonFields = [
    # application name
    "name",
    # mapping from ("application", "basename") -> to header artifact
    "includes",
    # references to ankers for the include directory
    "include_dir",
    # deps files short_path -> artifact
    "deps_files",
    # input mapping
    "input_mapping",
]

# target type to break circular dependencies
ErlangAppIncludeInfo = provider(
    fields = ErlangAppCommonFields,
)

ErlangAppInfo = provider(
    fields =
        ErlangAppCommonFields + [
            # version
            "version",

            # mapping from module name to beam artifact
            "beams",

            # for tests we need to preserve the private includes
            "private_includes",
            # mapping from name to dependency for all Erlang dependencies
            "dependencies",
            # Transitive Set for calculating the start order
            "start_dependencies",
            # reference to the .app file
            "app_file",
            # additional targets that the application depends on, the
            # default output will end up in priv/
            "resources",
            # references to ankers for the relevant directories for the application
            "priv_dir",
            "private_include_dir",
            "ebin_dir",
            # applications that are in path but not build by buck2 are virtual
            # the use-case for virtual apps are OTP applications that are shipeped
            # with the Erlang distribution
            "virtual",
            # app folders for all toolchain
            "app_folders",
            # app_folder for primary toolchain
            "app_folder",
        ],
)

ErlangReleaseInfo = provider(
    fields = [
        "name",
    ],
)

# toolchain provider
ErlangToolchainInfo = provider(
    fields = [
        "name",
        # command line erlc options used when compiling
        "erl_opts",
        # emulator flags used when calling erl
        "emu_flags",
        # struct containing the binaries erlc, escript, and erl
        # this is further split into local and RE
        "otp_binaries",
        # utility scripts
        # building .app file
        "app_file_script",
        # building escripts
        "escript_builder",
        # analyzing .(h|e)rl dependencies
        "dependency_analyzer",
        # trampoline rerouting stdout to stderr
        "erlc_trampoline",
        # name to parse_transform artifacts mapping for core parse_transforms (that are always used) and
        # user defines ones
        "core_parse_transforms",
        "parse_transforms",
        # filter spec for parse transforms
        "parse_transforms_filters",
        # release boot script builder
        "boot_script_builder",
        # build release_variables
        "release_variables_builder",
        # copying erts
        "include_erts",
        # edoc-generating escript
        "edoc",
        "edoc_options",
        # beams we need for various reasons
        "utility_modules",
        # env to be set for toolchain invocations
        "env",
    ],
)

# multi-version toolchain
ErlangMultiVersionToolchainInfo = provider(
    fields = [
        # toolchains
        "toolchains",
        # primary toolchain
        "primary",
    ],
)

# OTP Binaries
ErlangOTPBinariesInfo = provider(
    fields = [
        "erl",
        "erlc",
        "escript",
    ],
)

# parse_transform
ErlangParseTransformInfo = provider(
    fields = [
        # module implementing the parse_transform
        "source",
        # potential extra files placed in a resource folder
        "extra_files",
    ],
)

ErlangTestInfo = provider(
    fields =
        [
            # The name of the suite
            "name",
            # mapping from name to dependency for all Erlang dependencies
            "dependencies",
            # anchor to the output_dir
            "output_dir",
        ],
)
