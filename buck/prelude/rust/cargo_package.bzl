# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:prelude.bzl", "native")

DEFAULT_PLATFORM_TEMPLATES = {
    "linux-arm64": select({
        "DEFAULT": False,
        "config//os:linux": select({
            "DEFAULT": False,
            "config//cpu:arm64": True,
        }),
    }),
    "linux-x86_64": select({
        "DEFAULT": False,
        "config//os:linux": select({
            "DEFAULT": False,
            "config//cpu:x86_64": True,
        }),
    }),
    "macos-arm64": select({
        "DEFAULT": False,
        "config//os:macos": select({
            "DEFAULT": False,
            "config//cpu:arm64": True,
        }),
    }),
    "macos-x86_64": select({
        "DEFAULT": False,
        "config//os:macos": select({
            "DEFAULT": False,
            "config//cpu:x86_64": True,
        }),
    }),
    "windows-gnu": select({
        "DEFAULT": False,
        "config//os:windows": select({
            "DEFAULT": False,
            "config//abi:gnu": True,
        }),
    }),
    "windows-msvc": select({
        "DEFAULT": False,
        "config//os:windows": select({
            "DEFAULT": True,
            "config//abi:gnu": False,
            "config//abi:msvc": True,
        }),
    }),
}

def _select_map_recursive(sel, fn):
    return select_map(
        sel,
        lambda v: _select_map_recursive(v, fn) if type(v) == type(sel) else fn(v),
    )

def apply_platform_attrs(
        platform_attrs,
        universal_attrs = {},
        templates = DEFAULT_PLATFORM_TEMPLATES):
    combined_attrs = dict(universal_attrs)

    for platform, attrs in platform_attrs.items():
        template = templates.get(platform, None)
        if template:
            for attr, value in attrs.items():
                default_value = {} if type(value) == type({}) else [] if type(value) == type([]) else None
                conditional_value = _select_map_recursive(template, lambda cond: value if cond else default_value)
                if attr in combined_attrs:
                    combined_attrs[attr] = combined_attrs[attr] + conditional_value
                else:
                    combined_attrs[attr] = conditional_value

    return combined_attrs

def _cargo_rust_binary(name, platform = {}, **kwargs):
    kwargs = apply_platform_attrs(platform, kwargs)

    rustc_flags = kwargs.get("rustc_flags", [])
    kwargs["rustc_flags"] = ["--cap-lints=allow"] + rustc_flags

    native.rust_binary(name = name, **kwargs)

def _cargo_rust_library(name, platform = {}, **kwargs):
    kwargs = apply_platform_attrs(platform, kwargs)

    rustc_flags = kwargs.get("rustc_flags", [])
    kwargs["rustc_flags"] = ["--cap-lints=allow"] + rustc_flags

    kwargs.setdefault("doctests", False)

    # Support for reindeer's `python_ext` fixup is not implemented yet.
    kwargs.pop("dlopen_enable", None)
    kwargs.pop("linkable_alias", None)

    native.rust_library(name = name, **kwargs)

cargo = struct(
    rust_binary = _cargo_rust_binary,
    rust_library = _cargo_rust_library,
)
