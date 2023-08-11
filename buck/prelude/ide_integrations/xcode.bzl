# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

XCODE_DATA_SUB_TARGET = "xcode-data"
_XCODE_DATA_FILE_NAME = "xcode_data.json"

XcodeDataInfo = provider(fields = [
    "data",  # {str: _a}
])

def generate_xcode_data(
        ctx: AnalysisContext,
        rule_type: str,
        output: ["artifact", None],
        populate_rule_specific_attributes_func: ["function", None] = None,
        **kwargs) -> (list["DefaultInfo"], XcodeDataInfo.type):
    data = {
        "rule_type": rule_type,
        "target": ctx.label,
    }
    if output:
        data["output"] = output
    if populate_rule_specific_attributes_func:
        data.update(populate_rule_specific_attributes_func(ctx, **kwargs))

    json_file = ctx.actions.write_json(_XCODE_DATA_FILE_NAME, data)
    return [DefaultInfo(default_output = json_file)], XcodeDataInfo(data = data)
