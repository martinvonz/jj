# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

HttpArchiveExecDeps = provider(fields = [
    "create_exclusion_list",
    "exec_os_type",
])

def _http_archive_exec_deps_impl(ctx: AnalysisContext) -> list["provider"]:
    return [
        DefaultInfo(),
        HttpArchiveExecDeps(
            create_exclusion_list = ctx.attrs.create_exclusion_list,
            exec_os_type = ctx.attrs.exec_os_type,
        ),
    ]

http_archive_exec_deps = rule(
    impl = _http_archive_exec_deps_impl,
    attrs = {
        "create_exclusion_list": attrs.default_only(attrs.dep(default = "prelude//http_archive/tools:create_exclusion_list")),
        "exec_os_type": attrs.default_only(attrs.dep(default = "prelude//os_lookup/targets:os_lookup")),
    },
)
