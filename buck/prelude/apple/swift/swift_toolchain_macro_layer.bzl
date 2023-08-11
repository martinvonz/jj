# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

def swift_toolchain_macro_impl(swift_toolchain_rule = None, **kwargs):
    bitcode = read_root_config("swift", "bitcode")
    if bitcode != None:
        kwargs["object_format"] = "object"
        if bitcode.lower() == "true":
            kwargs["object_format"] = "bc"
        elif bitcode.lower() == "ir":
            kwargs["object_format"] = "ir"
        elif bitcode.lower() == "irgen":
            kwargs["object_format"] = "irgen"
        elif bitcode.lower() in ["embed", "embed-bitcode", "object-embed-bitcode"]:
            kwargs["object_format"] = "object-embed-bitcode"

    swift_toolchain_rule(
        **kwargs
    )
