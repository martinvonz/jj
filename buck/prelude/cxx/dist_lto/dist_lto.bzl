# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:paths.bzl", "paths")
load(
    "@prelude//cxx:cxx_bolt.bzl",
    "bolt",
    "cxx_use_bolt",
)
load(
    "@prelude//cxx:cxx_link_utility.bzl",
    "cxx_link_cmd",
    "linker_map_args",
)
load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxToolchainInfo")
load("@prelude//cxx:debug.bzl", "SplitDebugMode")
load(
    "@prelude//cxx:dwp.bzl",
    "run_dwp_action",
)
load(
    "@prelude//linking:link_info.bzl",
    "ArchiveLinkable",  # @unused Used as a type
    "FrameworksLinkable",  # @unused Used as a type
    "LinkArgs",
    "LinkableType",
    "LinkedObject",
    "ObjectsLinkable",  # @unused Used as a type
    "SharedLibLinkable",  # @unused Used as a type
    "append_linkable_args",
    "map_to_link_infos",
)
load("@prelude//utils:utils.bzl", "is_all")

_BitcodeLinkData = record(
    name = str,
    initial_object = "artifact",
    bc_file = "artifact",
    plan = "artifact",
    opt_object = "artifact",
)

_ArchiveLinkData = record(
    name = str,
    manifest = "artifact",
    # A file containing paths to artifacts that are known to reside in opt_objects_dir.
    opt_manifest = "artifact",
    objects_dir = "artifact",
    opt_objects_dir = "artifact",
    indexes_dir = "artifact",
    plan = "artifact",
    link_whole = bool,
    prepend = bool,
)

_DataType = enum(
    "bitcode",
    "archive",
    "cmd_args",
)

_IndexLinkData = record(
    data_type = _DataType.type,
    link_data = field([_BitcodeLinkData.type, _ArchiveLinkData.type]),
)

_PrePostFlags = record(
    pre_flags = list,
    post_flags = list,
)

def cxx_dist_link(
        ctx: AnalysisContext,
        links: list["LinkArgs"],
        # The destination for the link output.
        output: "artifact",
        linker_map: ["artifact", None] = None,
        # A category suffix that will be added to the category of the link action that is generated.
        category_suffix: [str, None] = None,
        # An identifier that will uniquely name this link action in the context of a category. Useful for
        # differentiating multiple link actions in the same rule.
        identifier: [str, None] = None,
        # This action will only happen if split_dwarf is enabled via the toolchain.
        generate_dwp: bool = True,
        executable_link: bool = True) -> LinkedObject.type:
    """
    Perform a distributed thin-lto link into the supplied output

    Distributed thinlto splits the link into three stages:
    1. global "indexing" step
    2. many individual compilation unit optimization steps
    3. final global link step

    The 2nd and 3rd of those are done just by constructing compiler/linker commands (in dynamic_output
    sections) using the output of the first.

    For the first, we need to post-process the linker index output to get it into a form
    that is easy for us to consume from within bzl.
    """

    def make_cat(c: str) -> str:
        """ Used to make sure categories for our actions include the provided suffix """
        if category_suffix != None:
            return c + "_" + category_suffix
        return c

    def make_id(i: str) -> str:
        """ Used to make sure identifiers for our actions include the provided identifier """
        if identifier != None:
            return identifier + "_" + i
        return i

    recorded_outputs = {}

    def name_for_obj(link_name: str, object_artifact: "artifact") -> str:
        """ Creates a unique name/path we can use for a particular object file input """
        prefix = "{}/{}".format(link_name, object_artifact.short_path)

        # it's possible (though unlikely) that we can get duplicate name/short_path, so just uniquify them
        if prefix in recorded_outputs:
            recorded_outputs[prefix] += 1
            extra = recorded_outputs[prefix]
            prefix = "{}-{}".format(prefix, extra)
        else:
            recorded_outputs[prefix] = 1
        return prefix

    names = {}

    def name_for_link(info: "LinkInfo") -> str:
        """ Creates a unique name for a LinkInfo that we are consuming """
        name = info.name or "unknown"
        if name not in names:
            names[name] = 1
        else:
            names[name] += 1
            name += "-{}".format(names[name])
        return make_id(name)

    links = [
        LinkArgs(
            tset = link.tset,
            flags = link.flags,
            infos = link.infos,
        )
        for link in links
    ]

    link_infos = map_to_link_infos(links)

    cxx_toolchain = ctx.attrs._cxx_toolchain[CxxToolchainInfo]
    lto_planner = cxx_toolchain.dist_lto_tools_info.planner
    lto_opt = cxx_toolchain.dist_lto_tools_info.opt
    lto_prepare = cxx_toolchain.dist_lto_tools_info.prepare
    lto_copy = cxx_toolchain.dist_lto_tools_info.copy

    PREPEND_ARCHIVE_NAMES = [
        # T130644072: If linked with `--whole-archive`, Clang builtins must be at the
        # front of the argument list to avoid conflicts with identically-named Rust
        # symbols from the `compiler_builtins` crate.
        #
        # Once linking of C++ binaries starts to use `rlib`s, this may no longer be
        # necessary, because our Rust `rlib`s won't need to contain copies of
        # `compiler_builtins` to begin with, unlike our Rust `.a`s which presently do
        # (T130789782).
        "clang_rt.builtins",
    ]

    # Information used to construct thinlto index link command:
    # Note: The index into index_link_data is important as it's how things will appear in
    # the plans produced by indexing. That allows us to map those indexes back to
    # the actual artifacts.
    index_link_data = []
    linkables_index = {}
    pre_post_flags = {}

    # buildifier: disable=uninitialized
    def add_linkable(idx: int, linkable: [ArchiveLinkable.type, SharedLibLinkable.type, ObjectsLinkable.type, FrameworksLinkable.type]):
        if idx not in linkables_index:
            linkables_index[idx] = [linkable]
        else:
            linkables_index[idx].append(linkable)

    # buildifier: disable=uninitialized
    def add_pre_post_flags(idx: int, flags: _PrePostFlags.type):
        if idx not in pre_post_flags:
            pre_post_flags[idx] = [flags]
        else:
            pre_post_flags[idx].append(flags)

    # Information used to construct the dynamic plan:
    plan_inputs = []
    plan_outputs = []

    # Information used to construct the opt dynamic outputs:
    archive_opt_manifests = []

    prepare_cat = make_cat("thin_lto_prepare")

    for link in link_infos:
        link_name = name_for_link(link)
        idx = len(index_link_data)

        add_pre_post_flags(idx, _PrePostFlags(
            pre_flags = link.pre_flags,
            post_flags = link.post_flags,
        ))

        for linkable in link.linkables:
            if linkable._type == LinkableType("objects"):
                add_linkable(idx, linkable)
                for obj in linkable.objects:
                    name = name_for_obj(link_name, obj)
                    bc_output = ctx.actions.declare_output(name + ".thinlto.bc")
                    plan_output = ctx.actions.declare_output(name + ".opt.plan")
                    opt_output = ctx.actions.declare_output(name + ".opt.o")

                    data = _IndexLinkData(
                        data_type = _DataType("bitcode"),
                        link_data = _BitcodeLinkData(
                            name = name,
                            initial_object = obj,
                            bc_file = bc_output,
                            plan = plan_output,
                            opt_object = opt_output,
                        ),
                    )
                    index_link_data.append(data)
                    plan_outputs.extend([bc_output, plan_output])
            elif linkable._type == LinkableType("archive") and linkable.supports_lto:
                # Our implementation of Distributed ThinLTO operates on individual objects, not archives. Since these
                # archives might still contain LTO-able bitcode, we first extract the objects within the archive into
                # another directory and write a "manifest" containing the list of objects that the archive contained.
                #
                # Later actions in the LTO compilation pipeline will read this manifest and dynamically dispatch
                # actions on the objects that the manifest reports.

                name = name_for_obj(link_name, linkable.archive.artifact)
                archive_manifest = ctx.actions.declare_output("%s/%s/manifest.json" % (prepare_cat, name))
                archive_objects = ctx.actions.declare_output("%s/%s/objects" % (prepare_cat, name), dir = True)
                archive_opt_objects = ctx.actions.declare_output("%s/%s/opt_objects" % (prepare_cat, name), dir = True)
                archive_indexes = ctx.actions.declare_output("%s/%s/indexes" % (prepare_cat, name), dir = True)
                archive_plan = ctx.actions.declare_output("%s/%s/plan.json" % (prepare_cat, name))
                archive_opt_manifest = ctx.actions.declare_output("%s/%s/opt_objects.manifest" % (prepare_cat, name))
                prepare_args = cmd_args([
                    lto_prepare,
                    "--manifest-out",
                    archive_manifest.as_output(),
                    "--objects-out",
                    archive_objects.as_output(),
                    "--ar",
                    cxx_toolchain.linker_info.archiver,
                    "--archive",
                    linkable.archive.artifact,
                    "--name",
                    name,
                ])
                ctx.actions.run(prepare_args, category = make_cat("thin_lto_prepare"), identifier = name)

                data = _IndexLinkData(
                    data_type = _DataType("archive"),
                    link_data = _ArchiveLinkData(
                        name = name,
                        manifest = archive_manifest,
                        opt_manifest = archive_opt_manifest,
                        objects_dir = archive_objects,
                        opt_objects_dir = archive_opt_objects,
                        indexes_dir = archive_indexes,
                        plan = archive_plan,
                        link_whole = linkable.link_whole,
                        prepend = link_name in PREPEND_ARCHIVE_NAMES,
                    ),
                )
                index_link_data.append(data)
                archive_opt_manifests.append(archive_opt_manifest)
                plan_inputs.extend([archive_manifest, archive_objects])
                plan_outputs.extend([archive_indexes, archive_plan])
            else:
                add_linkable(idx, linkable)
                index_link_data.append(None)

    index_argsfile_out = ctx.actions.declare_output(output.basename + ".thinlto.index.argsfile")
    final_link_index = ctx.actions.declare_output(output.basename + ".final_link_index")

    def dynamic_plan(link_plan: "artifact", index_argsfile_out: "artifact", final_link_index: "artifact"):
        def plan(ctx, artifacts, outputs):
            # buildifier: disable=uninitialized
            def add_pre_flags(idx: int):
                if idx in pre_post_flags:
                    for flags in pre_post_flags[idx]:
                        index_args.add(flags.pre_flags)

            # buildifier: disable=uninitialized
            def add_post_flags(idx: int):
                if idx in pre_post_flags:
                    for flags in pre_post_flags[idx]:
                        index_args.add(flags.post_flags)

            # buildifier: disable=uninitialized
            def add_linkables_args(idx: int):
                if idx in linkables_index:
                    object_link_arg = cmd_args()
                    for linkable in linkables_index[idx]:
                        append_linkable_args(object_link_arg, linkable)
                    index_args.add(object_link_arg)

            # index link command args
            prepend_index_args = cmd_args()
            index_args = cmd_args()

            # See comments in dist_lto_planner.py for semantics on the values that are pushed into index_meta.
            index_meta = cmd_args()

            # buildifier: disable=uninitialized
            for idx, artifact in enumerate(index_link_data):
                add_pre_flags(idx)
                add_linkables_args(idx)

                if artifact != None:
                    link_data = artifact.link_data

                    if artifact.data_type == _DataType("bitcode"):
                        index_meta.add(link_data.initial_object, outputs[link_data.bc_file].as_output(), outputs[link_data.plan].as_output(), str(idx), "", "", "")

                    elif artifact.data_type == _DataType("archive"):
                        manifest = artifacts[link_data.manifest].read_json()

                        if not manifest["objects"]:
                            # Despite not having any objects (and thus not needing a plan), we still need to bind the plan output.
                            ctx.actions.write(outputs[link_data.plan].as_output(), "{}")
                            cmd = cmd_args(["/bin/sh", "-c", "mkdir", "-p", outputs[link_data.indexes_dir].as_output()])
                            ctx.actions.run(cmd, category = make_cat("thin_lto_mkdir"), identifier = link_data.name)
                            continue

                        archive_args = prepend_index_args if link_data.prepend else index_args

                        archive_args.hidden(link_data.objects_dir)

                        if not link_data.link_whole:
                            archive_args.add("-Wl,--start-lib")

                        for obj in manifest["objects"]:
                            index_meta.add(obj, "", "", str(idx), link_data.name, outputs[link_data.plan].as_output(), outputs[link_data.indexes_dir].as_output())
                            archive_args.add(obj)

                        if not link_data.link_whole:
                            archive_args.add("-Wl,--end-lib")

                        archive_args.hidden(link_data.objects_dir)

                add_post_flags(idx)

            index_argfile, _ = ctx.actions.write(
                outputs[index_argsfile_out].as_output(),
                prepend_index_args.add(index_args),
                allow_args = True,
            )

            index_cat = make_cat("thin_lto_index")
            index_file_out = ctx.actions.declare_output(make_id(index_cat) + "/index")
            index_out_dir = cmd_args(index_file_out.as_output()).parent()

            index_cmd = cxx_link_cmd(ctx)
            index_cmd.add(cmd_args(index_argfile, format = "@{}"))

            output_as_string = cmd_args(output)
            output_as_string.ignore_artifacts()
            index_cmd.add("-o", output_as_string)
            index_cmd.add(cmd_args(index_file_out.as_output(), format = "-Wl,--thinlto-index-only={}"))
            index_cmd.add("-Wl,--thinlto-emit-imports-files")
            index_cmd.add("-Wl,--thinlto-full-index")
            index_cmd.add(cmd_args(index_out_dir, format = "-Wl,--thinlto-prefix-replace=;{}/"))

            # Terminate the index file with a newline.
            index_meta.add("")
            index_meta_file = ctx.actions.write(
                output.basename + ".thinlto.meta",
                index_meta,
            )

            plan_cmd = cmd_args([lto_planner, "--meta", index_meta_file, "--index", index_out_dir, "--link-plan", outputs[link_plan].as_output(), "--final-link-index", outputs[final_link_index].as_output(), "--"])
            plan_cmd.add(index_cmd)

            plan_extra_inputs = cmd_args()
            plan_extra_inputs.add(index_meta)
            plan_extra_inputs.add(index_args)
            plan_cmd.hidden(plan_extra_inputs)

            ctx.actions.run(plan_cmd, category = index_cat, identifier = identifier, local_only = True)

        # TODO(T117513091) - dynamic_output does not allow for an empty list of dynamic inputs. If we have no archives
        # to process, we will have no dynamic inputs, and the plan action can be non-dynamic.
        #
        # However, buck2 disallows `dynamic_output` with a empty input list. We also can't call our `plan` function
        # directly, since it uses `ctx.outputs` to bind its outputs. Instead of doing Starlark hacks to work around
        # the lack of `ctx.outputs`, we declare an empty file as a dynamic input.
        plan_inputs.append(ctx.actions.write(output.basename + ".plan_hack.txt", ""))
        plan_outputs.extend([link_plan, index_argsfile_out, final_link_index])
        ctx.actions.dynamic_output(dynamic = plan_inputs, inputs = [], outputs = plan_outputs, f = plan)

    link_plan_out = ctx.actions.declare_output(output.basename + ".link-plan.json")
    dynamic_plan(link_plan = link_plan_out, index_argsfile_out = index_argsfile_out, final_link_index = final_link_index)

    def prepare_opt_flags(link_infos: list["LinkInfo"]) -> cmd_args:
        opt_args = cmd_args()
        opt_args.add(cxx_link_cmd(ctx))

        # buildifier: disable=uninitialized
        for link in link_infos:
            for raw_flag in link.pre_flags + link.post_flags:
                opt_args.add(raw_flag)
        return opt_args

    opt_common_flags = prepare_opt_flags(link_infos)

    # We declare a separate dynamic_output for every object file. It would
    # maybe be simpler to have a single dynamic_output that produced all the
    # opt actions, but an action needs to re-run whenever the analysis that
    # produced it re-runs. And so, with a single dynamic_output, we'd need to
    # re-run all actions when any of the plans changed.
    def dynamic_optimize(name: str, initial_object: "artifact", bc_file: "artifact", plan: "artifact", opt_object: "artifact"):
        def optimize_object(ctx, artifacts, outputs):
            plan_json = artifacts[plan].read_json()

            # If the object was not compiled with thinlto flags, then there
            # won't be valid outputs for it from the indexing, but we still
            # need to bind the artifact.
            if not plan_json["is_bc"]:
                ctx.actions.write(outputs[opt_object], "")
                return

            opt_cmd = cmd_args(lto_opt)
            opt_cmd.add("--out", outputs[opt_object].as_output())
            opt_cmd.add("--input", initial_object)
            opt_cmd.add("--index", bc_file)

            # When invoking opt and llc via clang, clang will not respect IR metadata to generate
            # dwo files unless -gsplit-dwarf is explicitly passed in. In other words, even if
            # 'splitDebugFilename' set in IR 'DICompileUnit', we still need to force clang to tell
            # llc to generate dwo sections.
            #
            # Local thinlto generates .dwo files by default. For distributed thinlto, however, we
            # want to keep all dwo debug info in the object file to reduce the number of files to
            # materialize.
            if cxx_toolchain.split_debug_mode == SplitDebugMode("single"):
                opt_cmd.add("--split-dwarf=single")

            # Create an argsfile and dump all the flags to be processed later.
            opt_argsfile = ctx.actions.declare_output(outputs[opt_object].basename + ".opt.argsfile")
            ctx.actions.write(opt_argsfile.as_output(), opt_common_flags, allow_args = True)
            opt_cmd.hidden(opt_common_flags)
            opt_cmd.add("--args", opt_argsfile)

            opt_cmd.add("--")
            opt_cmd.add(cxx_toolchain.cxx_compiler_info.compiler)

            imports = [index_link_data[idx].link_data.initial_object for idx in plan_json["imports"]]
            archives = [index_link_data[idx].link_data.objects_dir for idx in plan_json["archive_imports"]]
            opt_cmd.hidden(imports)
            opt_cmd.hidden(archives)
            ctx.actions.run(opt_cmd, category = make_cat("thin_lto_opt"), identifier = name)

        ctx.actions.dynamic_output(dynamic = [plan], inputs = [], outputs = [opt_object], f = optimize_object)

    def dynamic_optimize_archive(archive: _ArchiveLinkData.type):
        def optimize_archive(ctx, artifacts, outputs):
            plan_json = artifacts[archive.plan].read_json()
            if "objects" not in plan_json or not plan_json["objects"] or is_all(lambda e: not e["is_bc"], plan_json["objects"]):
                # Nothing in this directory was lto-able; let's just copy the archive.
                ctx.actions.copy_file(outputs[archive.opt_objects_dir], archive.objects_dir)
                ctx.actions.write(outputs[archive.opt_manifest], "")
                return

            output_dir = {}
            output_manifest = cmd_args()
            for entry in plan_json["objects"]:
                base_dir = plan_json["base_dir"]
                source_path = paths.relativize(entry["path"], base_dir)
                if not entry["is_bc"]:
                    opt_object = ctx.actions.declare_output("%s/%s" % (make_cat("thin_lto_opt_copy"), source_path))
                    output_manifest.add(opt_object)
                    copy_cmd = cmd_args([
                        lto_copy,
                        "--to",
                        opt_object.as_output(),
                        "--from",
                        entry["path"],
                    ])

                    copy_cmd.hidden(archive.objects_dir)
                    ctx.actions.run(copy_cmd, category = make_cat("thin_lto_opt_copy"), identifier = source_path)
                    output_dir[source_path] = opt_object
                    continue

                opt_object = ctx.actions.declare_output("%s/%s" % (make_cat("thin_lto_opt"), source_path))
                output_manifest.add(opt_object)
                output_dir[source_path] = opt_object
                opt_cmd = cmd_args(lto_opt)
                opt_cmd.add("--out", opt_object.as_output())
                opt_cmd.add("--input", entry["path"])
                opt_cmd.add("--index", entry["bitcode_file"])

                if cxx_toolchain.split_debug_mode == SplitDebugMode("single"):
                    opt_cmd.add("--split-dwarf=single")

                opt_argsfile = ctx.actions.declare_output(opt_object.basename + ".opt.argsfile")
                ctx.actions.write(opt_argsfile.as_output(), opt_common_flags, allow_args = True)
                opt_cmd.add("--args", opt_argsfile)

                opt_cmd.add("--")
                opt_cmd.add(cxx_toolchain.cxx_compiler_info.compiler)

                imports = [index_link_data[idx].link_data.initial_object for idx in entry["imports"]]
                archives = [index_link_data[idx].link_data.objects_dir for idx in entry["archive_imports"]]
                opt_cmd.hidden(imports)
                opt_cmd.hidden(archives)
                opt_cmd.hidden(archive.indexes_dir)
                opt_cmd.hidden(archive.objects_dir)
                ctx.actions.run(opt_cmd, category = make_cat("thin_lto_opt"), identifier = source_path)

            ctx.actions.symlinked_dir(outputs[archive.opt_objects_dir], output_dir)
            ctx.actions.write(outputs[archive.opt_manifest], output_manifest, allow_args = True)

        archive_opt_inputs = [archive.plan]
        archive_opt_outputs = [archive.opt_objects_dir, archive.opt_manifest]
        ctx.actions.dynamic_output(dynamic = archive_opt_inputs, inputs = [], outputs = archive_opt_outputs, f = optimize_archive)

    for artifact in index_link_data:
        if artifact == None:
            continue
        link_data = artifact.link_data
        if artifact.data_type == _DataType("bitcode"):
            dynamic_optimize(
                name = link_data.name,
                initial_object = link_data.initial_object,
                bc_file = link_data.bc_file,
                plan = link_data.plan,
                opt_object = link_data.opt_object,
            )
        elif artifact.data_type == _DataType("archive"):
            dynamic_optimize_archive(link_data)

    linker_argsfile_out = ctx.actions.declare_output(output.basename + ".thinlto.link.argsfile")

    def thin_lto_final_link(ctx, artifacts, outputs):
        plan = artifacts[link_plan_out].read_json()
        link_args = cmd_args()
        plan_index = {int(k): v for k, v in plan["index"].items()}

        # non_lto_objects are the ones that weren't compiled with thinlto
        # flags. In that case, we need to link against the original object.
        non_lto_objects = {int(k): 1 for k in plan["non_lto_objects"]}
        current_index = 0
        opt_objects = []
        archives = []
        for link in link_infos:
            link_args.add(link.pre_flags)
            for linkable in link.linkables:
                if linkable._type == LinkableType("objects"):
                    new_objs = []
                    for obj in linkable.objects:
                        if current_index in plan_index:
                            new_objs.append(index_link_data[current_index].link_data.opt_object)
                            opt_objects.append(index_link_data[current_index].link_data.opt_object)
                        elif current_index in non_lto_objects:
                            new_objs.append(obj)
                            opt_objects.append(obj)
                        current_index += 1
                else:
                    current_index += 1
            link_args.add(link.post_flags)

        link_cmd = cxx_link_cmd(ctx)
        final_link_argfile, final_link_inputs = ctx.actions.write(
            outputs[linker_argsfile_out].as_output(),
            link_args,
            allow_args = True,
        )

        # buildifier: disable=uninitialized
        for artifact in index_link_data:
            if artifact != None and artifact.data_type == _DataType("archive"):
                link_cmd.hidden(artifact.link_data.opt_objects_dir)
        link_cmd.add(cmd_args(final_link_argfile, format = "@{}"))
        link_cmd.add(cmd_args(final_link_index, format = "@{}"))
        link_cmd.add("-o", outputs[output].as_output())
        if linker_map:
            link_cmd.add(linker_map_args(ctx, outputs[linker_map].as_output()).flags)
        link_cmd_extra_inputs = cmd_args()
        link_cmd_extra_inputs.add(final_link_inputs)
        link_cmd.hidden(link_cmd_extra_inputs)
        link_cmd.hidden(link_args)
        link_cmd.hidden(opt_objects)
        link_cmd.hidden(archives)

        ctx.actions.run(link_cmd, category = make_cat("thin_lto_link"), identifier = identifier, local_only = True)

    final_link_inputs = [link_plan_out, final_link_index] + archive_opt_manifests
    ctx.actions.dynamic_output(
        dynamic = final_link_inputs,
        inputs = [],
        outputs = [output] + ([linker_map] if linker_map else []) + [linker_argsfile_out],
        f = thin_lto_final_link,
    )

    final_output = output if not (executable_link and cxx_use_bolt(ctx)) else bolt(ctx, output, identifier)
    dwp_output = ctx.actions.declare_output(output.short_path.removesuffix("-wrapper") + ".dwp") if generate_dwp else None

    if generate_dwp:
        run_dwp_action(
            ctx = ctx,
            obj = final_output,
            identifier = identifier,
            category_suffix = category_suffix,
            referenced_objects = final_link_inputs,
            dwp_output = dwp_output,
            # distributed thinlto link actions are ran locally, run llvm-dwp locally as well to
            # ensure all dwo source files are available
            local_only = True,
        )

    return LinkedObject(
        output = final_output,
        prebolt_output = output,
        dwp = dwp_output,
        linker_argsfile = linker_argsfile_out,
        index_argsfile = index_argsfile_out,
    )
