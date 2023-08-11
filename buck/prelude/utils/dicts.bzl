# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(
    "@prelude//utils:utils.bzl",
    "expect",
)

_DEFAULT_FMT = "found different values for key \"{0}\": {} != {}"

def update_x(dst: dict["_a", "_b"], k: "_a", v: "_b", fmt = _DEFAULT_FMT):
    p = dst.setdefault(k, v)
    expect(p == v, fmt, k, p, v)

def merge_x(dst: dict["_a", "_b"], src: dict["_a", "_b"], fmt = _DEFAULT_FMT):
    for k, v in src.items():
        update_x(dst, k, v, fmt = fmt)

def flatten_x(ds: list[dict["_a", "_b"]], fmt = _DEFAULT_FMT) -> dict["_a", "_b"]:
    out = {}
    for d in ds:
        merge_x(out, d, fmt = fmt)
    return out
