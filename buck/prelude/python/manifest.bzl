# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:artifact_tset.bzl", "project_artifacts")
load(":toolchain.bzl", "PythonToolchainInfo")

# Manifests are files containing information how to map sources into a package.
# The files are JSON lists with an entry per source, where each source is 3-tuple
# of relative destination path, artifact path, and a short description of the
# origin of this source (used for error messages in tooling that uses these).
ManifestInfo = record(
    # The actual manifest file (in the form of a JSON file).
    manifest = field("artifact"),
    # All artifacts that are referenced in the manifest.
    artifacts = field([["artifact", "_arglike"]]),
)

# Parse imports from a *.py file to generate a list of required modules
def create_dep_manifest_for_source_map(
        ctx: AnalysisContext,
        python_toolchain: PythonToolchainInfo.type,
        srcs: dict[str, "artifact"]) -> ManifestInfo.type:
    entries = []
    artifacts = []
    for path, artifact in srcs.items():
        out_name = "__dep_manifests__/{}".format(path)
        if not path.endswith(".py"):
            continue

        dep_manifest = ctx.actions.declare_output(out_name)
        cmd = cmd_args(python_toolchain.parse_imports)
        cmd.add(cmd_args(artifact))
        cmd.add(cmd_args(dep_manifest.as_output()))
        ctx.actions.run(cmd, category = "generate_dep_manifest", identifier = out_name)
        entries.append((dep_manifest, path))
        artifacts.append(dep_manifest)

    manifest = ctx.actions.write_json("dep.manifest", entries)
    return ManifestInfo(
        manifest = manifest,
        artifacts = artifacts,
    )

def _write_manifest(
        ctx: AnalysisContext,
        name: str,
        entries: list[(str, "artifact", str)]) -> "artifact":
    """
    Serialize the given source manifest entries to a JSON file.
    """
    return ctx.actions.write_json(name + ".manifest", entries)

def create_manifest_for_entries(
        ctx: AnalysisContext,
        name: str,
        entries: list[(str, "artifact", str)]) -> ManifestInfo.type:
    """
    Generate a source manifest for the given list of sources.
    """
    return ManifestInfo(
        manifest = _write_manifest(ctx, name, entries),
        artifacts = [(a, dest) for dest, a, _ in entries],
    )

def create_manifest_for_source_map(
        ctx: AnalysisContext,
        param: str,
        srcs: dict[str, "artifact"]) -> ManifestInfo.type:
    """
    Generate a source manifest for the given map of sources from the given rule.
    """
    origin = "{} {}".format(ctx.label.raw_target(), param)
    return create_manifest_for_entries(
        ctx,
        param,
        [(dest, artifact, origin) for dest, artifact in srcs.items()],
    )

def create_manifest_for_source_dir(
        ctx: AnalysisContext,
        param: str,
        extracted: "artifact",
        exclude: [str, None]) -> ManifestInfo.type:
    """
    Generate a source manifest for the given directory of sources from the given
    rule.
    """
    manifest = ctx.actions.declare_output(param + ".manifest")
    cmd = cmd_args(ctx.attrs._create_manifest_for_source_dir[RunInfo])
    cmd.add("--origin={}".format(ctx.label.raw_target()))
    cmd.add(cmd_args(manifest.as_output(), format = "--output={}"))
    cmd.add(extracted)
    if exclude != None:
        cmd.add("--exclude={}".format(exclude))
    ctx.actions.run(cmd, category = "py_source_manifest", identifier = param)

    # TODO: enumerate directory?
    return ManifestInfo(manifest = manifest, artifacts = [(extracted, param)])

def create_manifest_for_extensions(
        ctx: AnalysisContext,
        extensions: dict[str, ("_a", Label)],
        # Whether to include DWP files.
        dwp: bool = False) -> ManifestInfo.type:
    entries = []
    for dest, (lib, label) in extensions.items():
        entries.append((dest, lib.output, str(label.raw_target())))
        if dwp and lib.dwp != None:
            entries.append((dest + ".dwp", lib.dwp, str(label.raw_target()) + ".dwp"))
    manifest = create_manifest_for_entries(ctx, "extensions", entries)

    # Include external debug paths, even though they're not explicitly listed
    # in the manifest, as python packaging may also consume debug paths which
    # were referenced in native code.
    for name, (lib, _) in extensions.items():
        for dbginfo in project_artifacts(ctx.actions, [lib.external_debug_info]):
            manifest.artifacts.append((dbginfo, name))

    return manifest
