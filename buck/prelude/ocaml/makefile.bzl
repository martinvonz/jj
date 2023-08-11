# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# Parse and resolve Makefile's produce by ocamldeps.
load("@prelude//:paths.bzl", "paths")

# [Note: Dynamic dependency calculations]
# ---------------------------------------
# We are given the contents of a Makefile produced by ocamldep in -native mode e.g.
# ```
#      quux.cmi:
#      quux.cmx:
#        quux.cmi
#      corge.cmx:
#        quux.cmx
#```
# and the sources we care about.
#
#'parse_makefile' produces a mapping of which sources depend on which. In fact,
# it computes a pair of mappings:
#
# Result (1):  Replace '.cmi' with '.mli' and '.cmx' with '.ml' e.g.
# ```
#      quux.mli:
#      quux.ml:
#        quux.mli
#      corge.ml:
#        quux.ml
# ```
# If -opqaue is not in effect then this reflects the dependency requirements
# that enable cross module inlining (see the description of the -opaque flag [in
# the ocamlopt manual](https://v2.ocaml.org/manual/native.html)).

# Result(2):
#
#   If '-opqaue' is not in effect, then this is a copy of result(1). If though
#   '-opaque' is in effect, replace 'foo.cmx' with 'foo.cmi' in the RHS of rules
#   as long as there is a rule with LHS 'foo.cmi' in the ocamldep input. Then,
#   replace '.cmi' with .mli' and '.cmx' with '.ml' as before e.g.
# ```
#      quux.mli:
#      quux.ml:
#        quux.mli
#      corge.ml:
#        quux.mli
# ```
# This mapping reflects the dependency requirements that don't admit cross
# module inlining (cutting down significantly on recompilations when only a
# module implementation and not its interface is changed).
def parse_makefile(makefile: str, srcs: list["artifact"], opaque_enabled: bool) -> (dict["artifact", list["artifact"]], dict["artifact", list["artifact"]]):
    # ocamldep is always invoked with '-native' so compiled modules are always
    # reported as '.cmx' files
    contents = _parse(makefile)

    # Collect all the files we reference.
    files = {}
    for aa, bb in contents:
        for x in aa + bb:
            files[x] = None

    # Produce a mapping of the files on the LHS of rules to their source
    # equivalents e.g. {'corge.cmx': 'corge.ml', 'quux.cmx': 'quux.ml',
    # 'quux.mli': 'quux.mli'}.
    resolved = _resolve(files.keys(), srcs)

    # This nested loop is technically O(n^2), but since `a` is at most 2 in OCaml
    # it's not an issue
    result = {}
    for aa, bb in contents:
        for a in aa:
            if a in resolved:
                result[resolved[a]] = [resolved[b] for b in bb if b in resolved]

    # This is not an optimization, we rely on this early return (see [Note:
    # Dynamic dependencies] earlier in this file).
    if not opaque_enabled:
        return (result, result)

    # Replace '.cmx' (or '.cmo') dependencies with '.cmi' on the RHS of
    # rules everywhere we can.
    contents = [_replace_rhs_cmx_with_cmi(contents, c) for c in contents]

    # Compute a new result based on having replaced '.cmx' ('.cmo') with '.cmi'
    # on the RHS of rules where we can.
    result2 = {}
    for aa, bb in contents:
        for a in aa:
            if a in resolved:
                result2[resolved[a]] = [resolved[b] for b in bb if b in resolved]

    return (result, result2)

def _resolve(entries: list[str], srcs: list["artifact"]) -> dict[str, "artifact"]:
    # O(n^2), alas - switch to a prefix map if that turns out to be too slow.
    result = {}
    for x in entries:
        prefix, ext = paths.split_extension(x)
        if ext == ".cmx":
            y = prefix + ".ml"
        elif ext == ".cmi":
            y = prefix + ".mli"
        else:
            fail("ocamldeps makefile, unexpected extension: " + x)
        possible = [src for src in srcs if y.endswith("/" + src.short_path)]
        if len(possible) == 1:
            result[x] = possible[0]
        elif len(possible) == 0:
            # If we get here, then ocamldep has found a dependency on a .ml file
            # that wasn't in `srcs`. On Remote Execution, that shouldn't happen,
            # and would be user error. When running locally, ocamldep will look
            # at any .ml files that are in the same directory as a `srcs` (no
            # way to stop it), so sometimes it finds those when they are meant
            # to be opaque or coming in through `deps`. The only solution is
            # just to ignore such unknown entries and "pretend" the .ml file
            # wasn't found.
            pass
        else:
            fail("ocamldeps makefile, multiple sources: " + x)
    return result

# Do the low level parse, returning the strings that are in the makefile as
# dependencies
def _parse(makefile: str) -> list[(list[str], list[str])]:
    # For the OCaml makefile, it's a series of lines, often with a '\' line
    # continuation.
    lines = []
    line = ""
    for x in makefile.splitlines():
        if x.endswith("\\"):
            line += x[:-1]
        else:
            lines.append(line + x)
            line = ""
    lines.append(line)

    result = []
    for x in lines:
        before, _, after = (x + " ").partition(" : ")
        result.append((before.split(), after.strip().split()))
    return result

# If 'f.cmi' appears on the LHS of a rule, change occurrences of 'f.cmx' to
# 'f.cmi' in all RHS's in the results of a parsed makefile.
def _replace_rhs_cmx_with_cmi(contents: list[(list[str], list[str])], p: (list[str], list[str])) -> (list[str], list[str]):
    def replace_in(p):
        pre, ext = paths.split_extension(p)

        # Search the LHS's of contents for a for 'p.cmi'.
        exists = len([p for p in contents if pre + ".cmi" in p[0]]) != 0
        if exists and ext == ".cmx":
            # Replace 'p.cmx' in this RHS with 'p.cmi'.
            return pre + ".cmi"
        return p

    xs, ys = p
    return (xs, [replace_in(y) for y in ys])
