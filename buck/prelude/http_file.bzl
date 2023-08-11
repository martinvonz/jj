# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//utils:utils.bzl", "expect", "value_or")

def http_file_shared(
        actions: "actions",
        name: str,
        url: str,
        vpnless_url: [None, str],
        is_executable: bool,
        is_exploded_zip: bool,
        unzip_tool: [RunInfo.type, None],
        sha1: [None, str],
        sha256 = [None, str]) -> list["provider"]:
    output = actions.declare_output(name)
    downloaded_output = actions.declare_output("exploded_zip") if is_exploded_zip else output
    actions.download_file(
        downloaded_output,
        url,
        vpnless_url = vpnless_url,
        is_executable = is_executable,
        sha1 = sha1,
        sha256 = sha256,
        is_deferrable = True,
    )

    if is_exploded_zip:
        actions.run(
            cmd_args([
                unzip_tool,
                "--src",
                downloaded_output,
                "--dst",
                output.as_output(),
            ]),
            category = "exploded_zip_unzip",
            local_only = sha1 == None,
        )

    providers = [DefaultInfo(default_output = output)]
    if is_executable:
        providers.append(RunInfo(args = [output]))
    return providers

def http_file_impl(ctx: AnalysisContext) -> list["provider"]:
    expect(len(ctx.attrs.urls) == 1, "multiple `urls` not supported: {}", ctx.attrs.urls)
    expect(len(ctx.attrs.vpnless_urls) < 2, "multiple `vpnless_urls` not supported: {}", ctx.attrs.vpnless_urls)
    if len(ctx.attrs.vpnless_urls) > 0:
        vpnless_url = ctx.attrs.vpnless_urls[0]
    else:
        vpnless_url = None
    return http_file_shared(
        ctx.actions,
        name = value_or(ctx.attrs.out, ctx.label.name),
        url = ctx.attrs.urls[0],
        vpnless_url = vpnless_url,
        sha1 = ctx.attrs.sha1,
        sha256 = ctx.attrs.sha256,
        is_executable = ctx.attrs.executable or False,
        is_exploded_zip = False,
        unzip_tool = None,
    )
