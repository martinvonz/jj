# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//configurations:rules.bzl", _config_implemented_rules = "implemented_rules")
load("@prelude//decls/common.bzl", "prelude_rule")
load("@prelude//open_source.bzl", "is_open_source")

# Combine the attributes we generate, we the custom implementations we have.
load("@prelude//rules_impl.bzl", "extra_attributes", "extra_implemented_rules", "rule_decl_records", "toolchain_rule_names", "transitions")

def _unimplemented(name, ctx):
    fail("Unimplemented rule type `{}` for target `{}`.".format(name, ctx.label))

def _unimplemented_impl(name):
    # We could use a lambda here, but then it means every single parse evaluates a lambda.
    # Lambda's have tricky semantics, so using partial lets us test Starlark prototypes with
    # some features disabled.
    return partial(_unimplemented, name)

def _mk_rule(rule_spec: "") -> "rule":
    name = rule_spec.name
    attributes = rule_spec.attrs

    # We want native code-containing rules to be marked incompatible with fat
    # platforms. Getting the ones that use cxx/apple toolchains is a little
    # overly broad as it includes things like python that don't themselves have
    # native code but need the toolchains if they depend on native code and in
    # that case incompatibility is transitive and they'll get it.
    fat_platform_compatible = True
    if name not in ("python_library", "python_binary", "python_test"):
        for toolchain_attr in ("_apple_toolchain", "_cxx_toolchain", "_go_toolchain"):
            if toolchain_attr in attributes:
                fat_platform_compatible = False

    # Fat platforms is an idea specific to our toolchains, so doesn't apply to
    # open source. Ideally this restriction would be done at the toolchain level.
    if is_open_source():
        fat_platform_compatible = True

    if not fat_platform_compatible:
        # copy so we don't try change the passed in object
        attributes = dict(attributes)
        attributes["_cxx_toolchain_target_configuration"] = attrs.dep(default = "fbcode//buck2/platform/execution:fat_platform_incompatible")

    extra_args = {}
    cfg = transitions.get(name)
    if cfg != None:
        extra_args["cfg"] = cfg

    if rule_spec.docs:
        doc = rule_spec.docs

        # This is awkward. When generating documentation, we'll strip leading whitespace
        # like it's a python docstring. For that to work here, we need the "Examples:" line
        # to match the other lines for leading whitespace. We've just hardcoded this to
        # be what its expected to be in prelude.
        # TODO(cjhopman): Figure out something better here.
        if rule_spec.examples:
            doc += "\n{}Examples:\n{}".format(" " * 8, rule_spec.examples)
        if rule_spec.further:
            doc += "\n{}Additional notes:\n{}".format(" " * 8, rule_spec.further)

        extra_args["doc"] = doc

    impl = rule_spec.impl
    extra_impl = getattr(extra_implemented_rules, name, None)
    if extra_impl:
        if impl:
            fail("{} had an impl in the declaration and in the extra_implemented_rules overrides".format(name))
        impl = extra_impl
    if not impl:
        impl = _unimplemented_impl(name)

    return rule(
        impl = impl,
        attrs = attributes,
        is_configuration_rule = name in _config_implemented_rules,
        is_toolchain_rule = name in toolchain_rule_names,
        **extra_args
    )

def _flatten_decls():
    decls = {}
    for decl_set in rule_decl_records:
        for rule in dir(decl_set):
            decls[rule] = getattr(decl_set, rule)
    return decls

def _update_rules(rules: dict[str, ""], extra_attributes: ""):
    for k in dir(extra_attributes):
        v = getattr(extra_attributes, k)
        if k in rules:
            d = dict(rules[k].attrs)
            d.update(v)
            rules[k] = prelude_rule(
                name = rules[k].name,
                impl = rules[k].impl,
                attrs = d,
                docs = rules[k].docs,
                examples = rules[k].examples,
                further = rules[k].further,
            )
        else:
            rules[k] = prelude_rule(
                name = k,
                impl = None,
                attrs = v,
                docs = None,
                examples = None,
                further = None,
            )

_declared_rules = _flatten_decls()
_update_rules(_declared_rules, extra_attributes)

rules = {rule.name: _mk_rule(rule) for rule in _declared_rules.values()}

# The rules are accessed by doing module.name, so we have to put them on the correct module.
load_symbols(rules)
