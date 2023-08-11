# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolsInfo")
load(
    "@prelude//linking:execution_preference.bzl",
    "LinkExecutionPreference",
    "LinkExecutionPreferenceDeterminatorInfo",
    "LinkExecutionPreferenceInfo",  # @unused Used as a type
    "get_action_execution_attributes",
)
load("@prelude//user:rule_spec.bzl", "RuleRegistrationSpec")
load(
    "@prelude//utils:build_target_pattern.bzl",
    "BuildTargetPattern",  # @unused Used as a type
    "parse_build_target_pattern",
)
load(
    "@prelude//utils:utils.bzl",
    "is_any",
)

_SelectionCriteria = record(
    include_build_target_patterns = field([BuildTargetPattern.type], []),
    include_regular_expressions = field(["regex"], []),
    exclude_build_target_patterns = field([BuildTargetPattern.type], []),
    exclude_regular_expressions = field(["regex"], []),
)

AppleSelectiveDebuggingInfo = provider(fields = [
    "scrub_binary",  # function
    "filter",  # function
])

AppleSelectiveDebuggingFilteredDebugInfo = record(
    map = field({"label": ["artifact"]}),
)

# The type of selective debugging json input to utilze.
_SelectiveDebuggingJsonTypes = [
    # Use a targets json file containing all targets to include.
    "targets",
    # Use a spec json file specifying the targets to include
    # and exclude via build target patterns and regular expressions.
    "spec",
]

_SelectiveDebuggingJsonType = enum(*_SelectiveDebuggingJsonTypes)

_LOCAL_LINK_THRESHOLD = 0.2

def _impl(ctx: AnalysisContext) -> list["provider"]:
    json_type = _SelectiveDebuggingJsonType(ctx.attrs.json_type)

    # process inputs and provide them up the graph with typing
    include_build_target_patterns = [parse_build_target_pattern(pattern) for pattern in ctx.attrs.include_build_target_patterns]
    include_regular_expressions = [experimental_regex(expression) for expression in ctx.attrs.include_regular_expressions]
    exclude_build_target_patterns = [parse_build_target_pattern(pattern) for pattern in ctx.attrs.exclude_build_target_patterns]
    exclude_regular_expressions = [experimental_regex(expression) for expression in ctx.attrs.exclude_regular_expressions]

    scrubber = ctx.attrs._apple_tools[AppleToolsInfo].selective_debugging_scrubber

    cmd = cmd_args(scrubber)
    if json_type == _SelectiveDebuggingJsonType("targets"):
        # If a targets json file is not provided, write an empty json file:
        targets_json_file = ctx.attrs.targets_json_file or ctx.actions.write_json("targets_json.txt", {"targets": []})
        cmd.add("--targets-file")
        cmd.add(targets_json_file)
    elif json_type == _SelectiveDebuggingJsonType("spec"):
        json_data = {
            "exclude_build_target_patterns": ctx.attrs.exclude_build_target_patterns,
            "exclude_regular_expressions": ctx.attrs.exclude_regular_expressions,
            "include_build_target_patterns": ctx.attrs.include_build_target_patterns,
            "include_regular_expressions": ctx.attrs.include_regular_expressions,
        }
        spec_file = ctx.actions.write_json("selective_debugging_spec.json", json_data)
        cmd.add("--spec-file")
        cmd.add(spec_file)
    else:
        fail("Expected json_type to be either `targets` or `spec`.")

    selection_criteria = _SelectionCriteria(
        include_build_target_patterns = include_build_target_patterns,
        include_regular_expressions = include_regular_expressions,
        exclude_build_target_patterns = exclude_build_target_patterns,
        exclude_regular_expressions = exclude_regular_expressions,
    )

    def scrub_binary(inner_ctx, executable: "artifact", executable_link_execution_preference: LinkExecutionPreference.type, adhoc_codesign_tool: ["RunInfo", None]) -> "artifact":
        inner_cmd = cmd_args(cmd)
        output = inner_ctx.actions.declare_output("debug_scrubbed/{}".format(executable.short_path))

        action_execution_properties = get_action_execution_attributes(executable_link_execution_preference)

        # If we're provided a codesign tool, provider it to the scrubber binary so that it may sign
        # the binary after scrubbing.
        if adhoc_codesign_tool:
            inner_cmd.add(["--adhoc-codesign-tool", adhoc_codesign_tool])
        inner_cmd.add(["--input", executable])
        inner_cmd.add(["--output", output.as_output()])
        inner_ctx.actions.run(
            inner_cmd,
            category = "scrub_binary",
            identifier = executable.short_path,
            prefer_local = action_execution_properties.prefer_local,
            prefer_remote = action_execution_properties.prefer_remote,
            local_only = action_execution_properties.local_only,
            force_full_hybrid_if_capable = action_execution_properties.full_hybrid,
        )
        return output

    def filter_debug_info(debug_info: "transitive_set_iterator") -> AppleSelectiveDebuggingFilteredDebugInfo.type:
        map = {}
        for infos in debug_info:
            for info in infos:
                if _is_label_included(info.label, selection_criteria):
                    map[info.label] = info.artifacts

        return AppleSelectiveDebuggingFilteredDebugInfo(map = map)

    def preference_for_links(links: list[Label], deps_preferences: list[LinkExecutionPreferenceInfo.type]) -> LinkExecutionPreference.type:
        # If any dependent links were run locally, prefer that the current link is also performed locally,
        # to avoid needing to upload the previous link.
        dep_prefered_local = is_any(lambda info: info.preference == LinkExecutionPreference("local"), deps_preferences)
        if dep_prefered_local:
            return LinkExecutionPreference("local")

        # If we're not provided a list of links, we can't make an informed determination.
        if not links:
            return LinkExecutionPreference("any")

        matching_links = filter(None, [link for link in links if _is_label_included(link, selection_criteria)])

        # If more than 20% of targets being linked are also downloaded for debugging, perform the
        # link locally, as we'd need to download the object files anyway (and can skip downloading the link output).
        # Otherwise, perform the link remotely, and we'll just download the debug data separately.
        if len(matching_links) / len(links) >= _LOCAL_LINK_THRESHOLD:
            return LinkExecutionPreference("local")
        return LinkExecutionPreference("remote")

    return [
        DefaultInfo(),
        AppleSelectiveDebuggingInfo(
            scrub_binary = scrub_binary,
            filter = filter_debug_info,
        ),
        LinkExecutionPreferenceDeterminatorInfo(preference_for_links = preference_for_links),
    ]

registration_spec = RuleRegistrationSpec(
    name = "apple_selective_debugging",
    impl = _impl,
    attrs = {
        "exclude_build_target_patterns": attrs.list(attrs.string(), default = []),
        "exclude_regular_expressions": attrs.list(attrs.string(), default = []),
        "include_build_target_patterns": attrs.list(attrs.string(), default = []),
        "include_regular_expressions": attrs.list(attrs.string(), default = []),
        "json_type": attrs.enum(_SelectiveDebuggingJsonTypes),
        "targets_json_file": attrs.option(attrs.source(), default = None),
        "_apple_tools": attrs.exec_dep(default = "fbsource//xplat/buck2/platform/apple:apple-tools", providers = [AppleToolsInfo]),
    },
)

def _is_label_included(label: Label, selection_criteria: _SelectionCriteria.type) -> bool:
    # If no include criteria are provided, we then include everything, as long as it is not excluded.
    if selection_criteria.include_build_target_patterns or selection_criteria.include_regular_expressions:
        if not _check_if_label_matches_patterns_or_expressions(label, selection_criteria.include_build_target_patterns, selection_criteria.include_regular_expressions):
            return False

    # If included (above snippet), ensure that this target is not excluded.
    return not _check_if_label_matches_patterns_or_expressions(label, selection_criteria.exclude_build_target_patterns, selection_criteria.exclude_regular_expressions)

def _check_if_label_matches_patterns_or_expressions(label: Label, patterns: list["BuildTargetPattern"], expressions: list["regex"]) -> bool:
    for pattern in patterns:
        if pattern.matches(label):
            return True
    for expression in expressions:
        if expression.match(str(label)):
            return True
    return False
