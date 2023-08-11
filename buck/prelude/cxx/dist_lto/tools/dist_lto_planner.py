#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

"""
A simple wrapper around a distributed thinlto index command to fit into buck2's
distributed thinlto build.

This reads in a couple of things:
    1. The "meta" file. This is a list of tuples of (object file, index output,
    plan output). All items are line-separated (so each tuple is three lines).
    2. The index and link plan output paths
    3. The commands for the actual index command.

It will invoke the index command and then copy the index outputs to the
requested locations and write a plan for each of those objects. This "plan" is
a simple json file with the most important thing being a list of the indices
of the imports needed for that file.

It will then additionally write a link plan, which is just a translation of
the thinlto index (which lists the objects actually needed for the final link).


Both opt and link plans use indices to refer to other files because it allows the bzl
code to easily map back to other objects held in buck memory.
"""

# pyre-unsafe

import argparse
import json
import os
import os.path
import subprocess
import sys
from typing import List


def _get_argsfile(args) -> str:
    # go through the flags passed to linker and find the index argsfile
    argsfiles = list(
        filter(lambda arg: arg.endswith("thinlto.index.argsfile"), args.index_args)
    )
    assert (
        len(argsfiles) == 1
    ), f"expect only 1 argsfile but seeing multiple ones: {argsfiles}"
    argsfile = argsfiles[0]
    if argsfile.startswith("@"):
        argsfile = argsfile[1:]
    return argsfile


def _extract_lib_search_path(argsfile_path: str) -> List[str]:
    lib_search_path = []
    with open(argsfile_path) as argsfile:
        for line in argsfile:
            if line.startswith("-L"):
                lib_search_path.append(line.strip())
    return lib_search_path


def main(argv):
    parser = argparse.ArgumentParser()
    parser.add_argument("--meta")
    parser.add_argument("--index")
    parser.add_argument("--link-plan")
    parser.add_argument("--final-link-index")
    parser.add_argument("index_args", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv[1:])

    subprocess.check_call(args.index_args[1:])

    bitcode_suffix = ".thinlto.bc"
    imports_suffix = ".imports"
    opt_objects_suffix = ".opt.o"  # please note the files are not exist yet, this is to generate the index file use in final link

    with open(args.meta) as meta:
        meta_lines = [line.strip() for line in meta.readlines()]

    def read_imports(path, imports_path):
        with open(imports_path) as infile:
            return [line.strip() for line in infile.readlines()]

    def index_path(path):
        return os.path.join(args.index, path)

    # The meta file comes directly from dist_lto.bzl and consists of a list of
    # 7-tuples of information. It is easiest for us to write each tuple member
    # as a separate line in Starlark, so these 7-tuples are encoded in groups
    # of seven lines.
    #
    # The seven pieces of information are:
    #  1. The path to the source bitcode file. This is used as an index into
    #     a dictionary (`mapping`) that records much of the metadata coming
    #     from these lines.
    #  2. The path to an output bitcode file. This script is expected to place a
    #     ThinLTO index file at this location (suffixed `.thinlto.bc`).
    #  3. The path to an output plan. This script is expected to place a link
    #     plan here (a JSON document indicating which other object files this)
    #     object file depends on, among other things.
    #  4. The link data's index in the Starlark array.
    #  5. If this object file came from an archive, the name of the archive. Otherwise,
    #     this line is empty.
    #  6. If this object file came from an archive, the path to an output plan.
    #     This script is expected to produce an archive link plan here (a JSON)
    #     document similar to the object link plan, except containing link
    #     information for every file in the archive from which this object
    #     came. Otherwise, this line is empty.
    #  7. If this object file came from an archive, the indexes directory of that
    #     archive. This script is expected to place all ThinLTO indexes derived
    #     from object files originating from this archive in that directory.
    #     Otherwise, this line is empty.
    #
    # There are two indices that are derived from this meta file: the object
    # index (mapping["index"]) and the archive index (mapping["archive_index"]).
    # These indices are indices into Starlark arrays for all objects and archive
    # linkables, respectively. This script does not inspect them.
    mapping = {}
    archives = {}
    for i in range(0, len(meta_lines), 7):
        path = meta_lines[i]
        output = meta_lines[i + 1]
        plan_output = meta_lines[i + 2]
        idx = int(meta_lines[i + 3])
        archive_name = meta_lines[i + 4]
        archive_plan = meta_lines[i + 5]
        archive_index_dir = meta_lines[i + 6]

        archive_idx = idx if output == "" else None  # archives do not have outputs
        mapping[path] = {
            "output": output,
            "plan_output": plan_output,
            "index": idx,
            "archive_index": archive_idx,
            "archive_name": archive_name,
        }
        if archive_idx is not None:
            archives[idx] = {
                "name": archive_name,
                "objects": [],
                "plan": archive_plan,
                "index_dir": archive_index_dir,
            }

    non_lto_objects = {}
    for path, data in sorted(mapping.items(), key=lambda v: v[0]):
        output_loc = data["output"]
        if os.path.exists(output_loc):
            continue

        if data["archive_index"] is not None:
            archives[data["archive_index"]]["objects"].append(path)
            continue

        bc_file = index_path(path) + bitcode_suffix
        imports_path = index_path(path) + imports_suffix
        os.makedirs(os.path.dirname(output_loc), exist_ok=True)

        if os.path.exists(imports_path):
            assert os.path.exists(bc_file), "missing bc file for %s" % path
            os.rename(bc_file, output_loc)
            imports = read_imports(path, imports_path)
            imports_list = []
            archives_list = []
            for path in imports:
                entry = mapping[path]
                if entry["archive_index"] is not None:
                    archives_list.append(int(entry["archive_index"]))
                else:
                    imports_list.append(entry["index"])
            plan = {
                "imports": imports_list,
                "archive_imports": archives_list,
                "index": data["index"],
                "bitcode_file": bc_file,
                "path": path,
                "is_bc": True,
            }
        else:
            non_lto_objects[data["index"]] = 1
            with open(output_loc, "w"):
                pass
            plan = {
                "is_bc": False,
            }

        with open(data["plan_output"], "w") as planout:
            json.dump(plan, planout, sort_keys=True)

    for archive in archives.values():
        # For archives, we must produce a plan that provides Starlark enough
        # information about how to launch a dynamic opt for each object file
        # in the archive.
        archive_plan = {}

        # This is convenient to store, since it's difficult for Starlark to
        # calculate it.
        archive_plan["base_dir"] = os.path.dirname(archive["plan"])
        object_plans = []
        for obj in archive["objects"]:
            imports_path = index_path(obj) + imports_suffix
            output_path = archive["index_dir"]
            os.makedirs(output_path, exist_ok=True)
            if os.path.exists(imports_path):
                bc_file = index_path(obj) + bitcode_suffix
                os.rename(bc_file, os.path.join(output_path, os.path.basename(bc_file)))
                imports = read_imports(path, imports_path)

                imports_list = []
                archives_list = []
                for path in imports:
                    entry = mapping[path]
                    if entry["archive_index"] is not None:
                        archives_list.append(int(entry["archive_index"]))
                    else:
                        imports_list.append(entry["index"])
                object_plans.append(
                    {
                        "is_bc": True,
                        "path": obj,
                        "imports": imports_list,
                        "archive_imports": archives_list,
                        "bitcode_file": os.path.join(
                            output_path, os.path.basename(bc_file)
                        ),
                    }
                )
            else:
                object_plans.append(
                    {
                        "is_bc": False,
                        "path": obj,
                    }
                )

        archive_plan["objects"] = object_plans
        with open(archive["plan"], "w") as planout:
            json.dump(archive_plan, planout, sort_keys=True)

    # We read the `index`` and `index.full`` files produced by linker in index stage
    # and translate them to 2 outputs:
    # 1. A link plan build final_link args. (This one may be able to be removed if we refactor the workflow)
    # 2. A files list (*.final_link_index) used for final link stage which includes all the
    #    files needed. it's based on index.full with some modification, like path updates
    #    and redundant(added by toolchain) dependencies removing.
    index = {}
    index_files_set = set()
    # TODO(T130322878): since we call linker wrapper twice (in index and in final_link), to avoid these libs get
    # added twice we remove them from the index file for now.
    KNOWN_REMOVABLE_DEPS_SUFFIX = [
        "glibc/lib/crt1.o",
        "glibc/lib/crti.o",
        "glibc/lib/Scrt1.o",
        "crtbegin.o",
        "crtbeginS.o",
        ".build_info.o",
        "crtend.o",
        "crtendS.o",
        "glibc/lib/crtn.o",
    ]
    with open(index_path("index")) as indexfile:
        for line in indexfile:
            line = line.strip()
            index_files_set.add(line)
            path = os.path.relpath(line, start=args.index)
            index[mapping[path]["index"]] = 1

    with open(args.link_plan, "w") as outfile:
        json.dump(
            {
                "non_lto_objects": non_lto_objects,
                "index": index,
            },
            outfile,
            indent=2,
            sort_keys=True,
        )

    # Append all search path flags (e.g -Lfbcode/third-party-buck/platform010/build/glibc/lib) from argsfile to final_index
    # this workaround is to make dist_lto compatible with link_group. see T136415235 for more info
    argsfile = _get_argsfile(args)
    lib_search_path = _extract_lib_search_path(argsfile)

    # build index file for final link use
    with open(index_path("index.full")) as full_index_input, open(
        args.final_link_index, "w"
    ) as final_link_index_output:
        final_link_index_output.write("\n".join(lib_search_path) + "\n")
        for line in full_index_input:
            line = line.strip()
            if any(filter(line.endswith, KNOWN_REMOVABLE_DEPS_SUFFIX)):
                continue
            path = os.path.relpath(line, start=args.index)
            if line in index_files_set:
                if mapping[path]["output"]:
                    # handle files that were not extracted from archives
                    output = mapping[path]["output"].replace(
                        bitcode_suffix, opt_objects_suffix
                    )
                    final_link_index_output.write(output + "\n")
                elif os.path.exists(index_path(path) + imports_suffix):
                    # handle files built from source that were extracted from archives
                    opt_objects_path = path.replace(
                        "/objects/", "/opt_objects/objects/"
                    )
                    final_link_index_output.write(opt_objects_path + "\n")
                else:
                    # handle pre-built archives
                    final_link_index_output.write(line + "\n")
            else:
                # handle input files that did not come from linker input, e.g. linkerscirpts
                final_link_index_output.write(line + "\n")


sys.exit(main(sys.argv))
