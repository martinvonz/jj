# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolsInfo")
load("@prelude//user:rule_spec.bzl", "RuleRegistrationSpec")

def _impl(ctx: AnalysisContext) -> list["provider"]:
    return [
        DefaultInfo(),
        AppleToolsInfo(
            assemble_bundle = ctx.attrs.assemble_bundle[RunInfo],
            split_arch_combine_dsym_bundles_tool = ctx.attrs.split_arch_combine_dsym_bundles_tool[RunInfo],
            dry_codesign_tool = ctx.attrs.dry_codesign_tool[RunInfo],
            adhoc_codesign_tool = ctx.attrs.adhoc_codesign_tool[RunInfo],
            info_plist_processor = ctx.attrs.info_plist_processor[RunInfo],
            make_modulemap = ctx.attrs.make_modulemap[RunInfo],
            make_vfsoverlay = ctx.attrs.make_vfsoverlay[RunInfo],
            selective_debugging_scrubber = ctx.attrs.selective_debugging_scrubber[RunInfo],
            swift_objc_header_postprocess = ctx.attrs.swift_objc_header_postprocess[RunInfo],
        ),
    ]

# The `apple_tools` rule exposes a set of supplementary tools
# required by the Apple rules _internally_. Such tools are not
# toolchain/SDK specific, they're just internal helper tools.
registration_spec = RuleRegistrationSpec(
    name = "apple_tools",
    impl = _impl,
    attrs = {
        "adhoc_codesign_tool": attrs.dep(providers = [RunInfo]),
        "assemble_bundle": attrs.dep(providers = [RunInfo]),
        "dry_codesign_tool": attrs.dep(providers = [RunInfo]),
        "info_plist_processor": attrs.dep(providers = [RunInfo]),
        "make_modulemap": attrs.dep(providers = [RunInfo]),
        "make_vfsoverlay": attrs.dep(providers = [RunInfo]),
        "selective_debugging_scrubber": attrs.dep(providers = [RunInfo]),
        "split_arch_combine_dsym_bundles_tool": attrs.dep(providers = [RunInfo]),
        "swift_objc_header_postprocess": attrs.dep(providers = [RunInfo]),
    },
)
