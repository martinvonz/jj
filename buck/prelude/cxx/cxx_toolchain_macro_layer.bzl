# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

def cxx_toolchain_macro_impl(cxx_toolchain_rule = None, **kwargs):
    # `cxx.linker_map_enabled` overrides toolchain behavior
    linker_map_enabled = read_root_config("cxx", "linker_map_enabled")
    if linker_map_enabled != None:
        if linker_map_enabled.lower() == "true":
            kwargs["generate_linker_maps"] = True
        else:
            kwargs["generate_linker_maps"] = False

    bitcode = read_root_config("cxx", "bitcode")
    if bitcode != None:
        if bitcode.lower() == "false":
            kwargs["object_format"] = "native"
        elif bitcode.lower() == "true":
            kwargs["object_format"] = "bitcode"
        elif bitcode.lower() == "embed":
            kwargs["object_format"] = "embedded-bitcode"
        else:
            kwargs["object_format"] = "native"

    cxx_toolchain_rule(
        **kwargs
    )
