# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# `apple_bundle.info_plist_substitutions` might contain `CODE_SIGN_ENTITLEMENTS` key which (as per v1 documentation):
#
# > Code signing will embed entitlements pointed to by the entitlements_file arg in the bundle's apple_binary.
# > This is the preferred way to specify entitlements when building with Buck.
# > If the entitlements file is not present, it falls back to the CODE_SIGN_ENTITLEMENTS entry in info_plist_substitutions.
#
# In order to properly depend on this fallback entitlements file (and manipulate it) we have to convert this text entry into the source artifact.
# We only can do that on macro layer, hence the purpose of the following code.

_SOURCE_ROOT_PREFIX = "$(SOURCE_ROOT)/"
_CODE_SIGN_ENTITLEMENTS_KEY = "CODE_SIGN_ENTITLEMENTS"

def _find_first_variable(string: str) -> [(str, (str, str)), None]:
    """
    If variable like `$(FOO)` is not found in `string` returns `None`, else returns tuple
    with first element equal to variable name (e.g. `FOO`) and second element equal to tuple
    of part before and after this variable.
    """
    expansion_start = "$("
    expansion_end = ")"
    variable_start = string.find(expansion_start)
    if variable_start == -1:
        return None
    variable_end = string.find(expansion_end, variable_start)
    if variable_end == -1:
        fail("Expected variable expansion in string: `{}`".format(string))
    variable = string[variable_start + len(expansion_start):variable_end - len(expansion_end) + 1]
    prefix = string[:variable_start]
    suffix = string[variable_end + 1:]
    return (variable, (prefix, suffix))

def _expand_codesign_entitlements_path(info_plist_substitutions: dict[str, str], path: str) -> str:
    path = path.strip()
    for _ in range(100):
        if path.startswith(_SOURCE_ROOT_PREFIX):
            path = path[len(_SOURCE_ROOT_PREFIX):]
        maybe_variable = _find_first_variable(path)
        if not maybe_variable:
            return path
        (key, (prefix, suffix)) = maybe_variable
        maybe_value = info_plist_substitutions.get(key)
        if not maybe_value:
            fail("Expected to find value for `{}` in `info_plist_substitutions` dictionary `{}`".format(key, info_plist_substitutions))
        path = prefix + maybe_value + suffix
    fail("Too many iteration (loop might be present) to expand `{}` with substitutions `{}`".format(path, info_plist_substitutions))

def parse_codesign_entitlements(info_plist_substitutions: [dict[str, str], None]) -> [str, None]:
    if not info_plist_substitutions:
        return None
    maybe_path = info_plist_substitutions.get(_CODE_SIGN_ENTITLEMENTS_KEY)
    if not maybe_path:
        return None
    return _expand_codesign_entitlements_path(info_plist_substitutions, maybe_path)
