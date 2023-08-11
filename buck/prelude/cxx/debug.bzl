# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# Model the various "split" debug scenarios (e.g. `-gsplit-dwarf`).
SplitDebugMode = enum(
    # Debug info, if present, is inline in the object file, and will be linked
    # into executables and shared libraries (e.g. traditional behavior when
    # using `-g`).
    "none",
    # Debug info. if present is included in the object file, but will *not* be
    # linked into executables and shared libraries.  This style usually requires
    # an additional step, separate from the link, to combine and package debug
    # info (e.g. `dSYM`, `dwp`).
    "single",
    # FIXME(agallagher): Add support for "split", which probably just requires
    # modifying `compile_cxx` to add a `.dwo` file as a hidden output in this
    # case.
    #"split",
)
