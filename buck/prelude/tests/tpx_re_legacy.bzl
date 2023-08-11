# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//utils:utils.bzl", "expect")

_RE_ENABLED = "supports_remote_execution"
_RE_OPTS_LABEL_PREFIX = "re_opts_capabilities="
_RE_OPTS_KEYS = ["platform", "subplatform"]

def _parse_re_opts(labels: list[str]) -> [dict[str, str], None]:
    """
    Parse out JSON-embedded RE options like:
    "re_opts_capabilities={\"platform\": \"gpu-remote-execution\", \"subplatform\": \"P100\"}"
    """

    for label in labels:
        if label.startswith(_RE_OPTS_LABEL_PREFIX):
            result = json.decode(label[len(_RE_OPTS_LABEL_PREFIX):])
            for key in result.keys():
                expect(key in _RE_OPTS_KEYS, "unexpected key in RE options label: {}", key)
            return result

    return None

# TODO(agallagher): Parsing RE options via JSON embedded in labels isn't a great
# UI, and we just do it here to support existing use cases.  Ideally, though, we'd
# present a better UI (e.g. an `re_opts` param for tests) and use that instead.
# TODO(nga): remove "command_executor_config_builder", this is dead code after the version bump.
def get_re_executor_from_labels(labels: list[str]) -> ["command_executor_config_builder", "command_executor_config", None]:
    """
    Parse legacy RE-enablement test labels and use them to configure a test RE
    executor to run the test with.

    The UI is best documented at:
    https://www.internalfb.com/intern/wiki/Remote_Execution/Users/GPU_RE_Contbuild_Migration/
    """

    # If the special "RE enabled" label isn't present, abort.
    if _RE_ENABLED not in labels:
        return None

    # If there's no options found in labels, don't use RE.  This diverges from
    # v1 behavior, but v2+tpx needs some platform to be set and so we probably
    # want to the toolchain tp provide some exec-platform compatible platform.
    re_opts = _parse_re_opts(labels)
    if re_opts == None:
        return None

    return CommandExecutorConfig(
        local_enabled = False,
        remote_enabled = True,
        remote_execution_properties = re_opts,
        remote_execution_use_case = "tpx-default",
    )
