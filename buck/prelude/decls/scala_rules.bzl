# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(":common.bzl", "AbiGenerationMode", "ForkMode", "LogLevel", "SourceAbiVerificationMode", "TestType", "UnusedDependenciesAction", "prelude_rule")

scala_library = prelude_rule(
    name = "scala_library",
    docs = "",
    examples = None,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "abi_generation_mode": attrs.option(attrs.enum(AbiGenerationMode), default = None),
            "annotation_processor_deps": attrs.list(attrs.dep(), default = []),
            "annotation_processor_params": attrs.list(attrs.string(), default = []),
            "annotation_processors": attrs.list(attrs.string(), default = []),
            "contacts": attrs.list(attrs.string(), default = []),
            "default_host_platform": attrs.option(attrs.configuration_label(), default = None),
            "deps": attrs.list(attrs.dep(), default = []),
            "exported_deps": attrs.list(attrs.dep(), default = []),
            "exported_provided_deps": attrs.list(attrs.dep(), default = []),
            "extra_arguments": attrs.list(attrs.string(), default = []),
            "java_version": attrs.option(attrs.string(), default = None),
            "javac": attrs.option(attrs.source(), default = None),
            "labels": attrs.list(attrs.string(), default = []),
            "licenses": attrs.list(attrs.source(), default = []),
            "manifest_file": attrs.option(attrs.source(), default = None),
            "maven_coords": attrs.option(attrs.string(), default = None),
            "never_mark_as_unused_dependency": attrs.option(attrs.bool(), default = None),
            "on_unused_dependencies": attrs.option(attrs.enum(UnusedDependenciesAction), default = None),
            "plugins": attrs.list(attrs.dep(), default = []),
            "proguard_config": attrs.option(attrs.source(), default = None),
            "provided_deps": attrs.list(attrs.dep(), default = []),
            "remove_classes": attrs.list(attrs.regex(), default = []),
            "required_for_source_only_abi": attrs.bool(default = False),
            "resources": attrs.list(attrs.source(), default = []),
            "resources_root": attrs.option(attrs.string(), default = None),
            "runtime_deps": attrs.list(attrs.dep(), default = []),
            "source": attrs.option(attrs.string(), default = None),
            "source_abi_verification_mode": attrs.option(attrs.enum(SourceAbiVerificationMode), default = None),
            "source_only_abi_deps": attrs.list(attrs.dep(), default = []),
            "srcs": attrs.list(attrs.source(), default = []),
            "target": attrs.option(attrs.string(), default = None),
        }
    ),
)

scala_test = prelude_rule(
    name = "scala_test",
    docs = "",
    examples = None,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "abi_generation_mode": attrs.option(attrs.enum(AbiGenerationMode), default = None),
            "annotation_processor_deps": attrs.list(attrs.dep(), default = []),
            "annotation_processor_params": attrs.list(attrs.string(), default = []),
            "annotation_processors": attrs.list(attrs.string(), default = []),
            "contacts": attrs.list(attrs.string(), default = []),
            "cxx_library_whitelist": attrs.list(attrs.dep(), default = []),
            "default_cxx_platform": attrs.option(attrs.string(), default = None),
            "default_host_platform": attrs.option(attrs.configuration_label(), default = None),
            "deps": attrs.list(attrs.dep(), default = []),
            "deps_query": attrs.option(attrs.query(), default = None),
            "env": attrs.dict(key = attrs.string(), value = attrs.arg(), sorted = False, default = {}),
            "exported_deps": attrs.list(attrs.dep(), default = []),
            "exported_provided_deps": attrs.list(attrs.dep(), default = []),
            "extra_arguments": attrs.list(attrs.string(), default = []),
            "fork_mode": attrs.enum(ForkMode, default = "none"),
            "java_version": attrs.option(attrs.string(), default = None),
            "javac": attrs.option(attrs.source(), default = None),
            "labels": attrs.list(attrs.string(), default = []),
            "licenses": attrs.list(attrs.source(), default = []),
            "manifest_file": attrs.option(attrs.source(), default = None),
            "maven_coords": attrs.option(attrs.string(), default = None),
            "never_mark_as_unused_dependency": attrs.option(attrs.bool(), default = None),
            "on_unused_dependencies": attrs.option(attrs.enum(UnusedDependenciesAction), default = None),
            "plugins": attrs.list(attrs.dep(), default = []),
            "proguard_config": attrs.option(attrs.source(), default = None),
            "provided_deps": attrs.list(attrs.dep(), default = []),
            "remove_classes": attrs.list(attrs.regex(), default = []),
            "required_for_source_only_abi": attrs.bool(default = False),
            "resources": attrs.list(attrs.source(), default = []),
            "resources_root": attrs.option(attrs.string(), default = None),
            "run_test_separately": attrs.bool(default = False),
            "runtime_deps": attrs.list(attrs.dep(), default = []),
            "source": attrs.option(attrs.string(), default = None),
            "source_abi_verification_mode": attrs.option(attrs.enum(SourceAbiVerificationMode), default = None),
            "source_only_abi_deps": attrs.list(attrs.dep(), default = []),
            "srcs": attrs.list(attrs.source(), default = []),
            "std_err_log_level": attrs.option(attrs.one_of(attrs.enum(LogLevel), attrs.int()), default = None),
            "std_out_log_level": attrs.option(attrs.one_of(attrs.enum(LogLevel), attrs.int()), default = None),
            "target": attrs.option(attrs.string(), default = None),
            "test_case_timeout_ms": attrs.option(attrs.int(), default = None),
            "test_rule_timeout_ms": attrs.option(attrs.int(), default = None),
            "test_type": attrs.option(attrs.enum(TestType), default = None),
            "use_cxx_libraries": attrs.option(attrs.bool(), default = None),
            "use_dependency_order_classpath": attrs.option(attrs.bool(), default = None),
            "vm_args": attrs.list(attrs.arg(), default = []),
        }
    ),
)

scala_rules = struct(
    scala_library = scala_library,
    scala_test = scala_test,
)
