# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# Implementation of the `export_file` build rule.

def export_file_impl(ctx: AnalysisContext) -> list[DefaultInfo.type]:
    # mode is "copy" or "reference", defaulting to copy
    copy = ctx.attrs.mode != "reference"

    if copy:
        dest = ctx.label.name if ctx.attrs.out == None else ctx.attrs.out
        output = ctx.actions.copy_file(dest, ctx.attrs.src)
    elif ctx.attrs.out != None:
        fail("export_file does not allow specifying `out` without also specifying `mode = 'copy'`")
    else:
        output = ctx.attrs.src
    return [DefaultInfo(default_output = output)]
