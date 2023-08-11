# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

def get_re_executor_from_props(re_props: [dict[str, [dict[str, str], str, bool, None]], None]) -> ["command_executor_config", None]:
    """
    Convert the `remote_execution` properties param into a `CommandExecutorConfig`
    to use with test providers.
    """

    if re_props == None:
        return None

    re_props_copy = dict(re_props)
    capabilities = re_props_copy.pop("capabilities")
    use_case = re_props_copy.pop("use_case")
    remote_cache_enabled = re_props_copy.pop("remote_cache_enabled", None)
    if re_props_copy:
        unexpected_props = ", ".join(re_props_copy.keys())
        fail("found unexpected re props: " + unexpected_props)

    return CommandExecutorConfig(
        local_enabled = False,
        remote_enabled = True,
        remote_execution_properties = capabilities,
        remote_execution_use_case = use_case or "tpx-default",
        remote_cache_enabled = remote_cache_enabled,
    )
