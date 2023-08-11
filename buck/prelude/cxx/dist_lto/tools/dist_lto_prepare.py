#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

"""
Prepares for an object-only ThinLTO link by extracting a given archive and
producing a manifest of the objects contained within.
"""

import argparse
import enum
import json
import os
import subprocess
import sys
from typing import List, Tuple


class ArchiveKind(enum.IntEnum):
    UNKNOWN = 0
    ARCHIVE = 1
    THIN_ARCHIVE = 2


def _gen_path(parent_path: str, filename: str) -> str:
    # concat file path and check the file exist before return
    obj_path = os.path.join(parent_path, filename)
    assert os.path.exists(obj_path)
    return obj_path


def _gen_filename(filename: str, num_of_instance: int) -> str:
    # generate the filename based on the instance,
    # for 1st instance, it's file.o
    # for 2nd instance, it's file_1.o
    if num_of_instance > 1:
        basename, extension = os.path.splitext(filename)
        return f"{basename}_{num_of_instance-1}{extension}"
    else:
        return filename


def identify_file(path: str) -> Tuple[ArchiveKind, str]:
    path = os.path.realpath(path)
    output = subprocess.check_output(["file", "-b", path]).decode()
    if "ar archive" in output:
        return (ArchiveKind.ARCHIVE, output)
    elif "thin archive" in output:
        return (ArchiveKind.THIN_ARCHIVE, output)
    else:
        with open(path, "rb") as infile:
            head = infile.read(7)

            if head == "!<thin>".encode():
                return (ArchiveKind.THIN_ARCHIVE, output)

    return (ArchiveKind.UNKNOWN, output)


def main(argv: List[str]) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest-out")
    parser.add_argument("--objects-out")
    parser.add_argument("--ar")
    parser.add_argument("--name")
    parser.add_argument("--archive")
    args = parser.parse_args(argv[1:])

    objects_path = args.objects_out
    os.makedirs(objects_path, exist_ok=True)

    known_objects = []
    file_type, debug_output = identify_file(args.archive)
    if file_type == ArchiveKind.ARCHIVE:
        # Unfortunately, we use llvm-ar and, while binutils ar has had --output for
        # a long time, llvm-ar does not support --output and the change in llvm-ar
        # looks like it has stalled for years (https://reviews.llvm.org/D69418)
        # So, we need to invoke ar in the directory that we want it to extract into, and so
        # need to adjust some paths.
        ar_path = os.path.relpath(args.ar, start=objects_path)
        archive_path = os.path.relpath(args.archive, start=objects_path)
        output = subprocess.check_output(
            [ar_path, "t", archive_path], cwd=objects_path
        ).decode()
        member_list = [member for member in output.split("\n") if member]

        # no duplicated filename
        output = subprocess.check_output(
            [ar_path, "xv", archive_path], cwd=objects_path
        ).decode()
        for line in output.splitlines():
            assert line.startswith("x - ")
            obj = line[4:]
            known_objects.append(_gen_path(objects_path, obj))

        # Count all members of the same name.
        counter = {}
        for member in member_list:
            counter.setdefault(member, 0)
            counter[member] += 1

        for member, count in counter.items():
            if count <= 1:
                continue
            for current in range(1, count + 1):
                if current == 1:
                    # just extract the file
                    output = subprocess.check_output(
                        [ar_path, "xN", str(current), archive_path, member],
                        cwd=objects_path,
                    ).decode()
                    assert not output
                    # We've already added this above.
                else:
                    # llvm doesn't allow --output so we need this clumsiness
                    tmp_filename = "tmp"
                    current_file = _gen_filename(member, current)
                    # rename current 'member' file to tmp
                    output = subprocess.check_output(
                        ["mv", member, tmp_filename], cwd=objects_path
                    ).decode()
                    assert not output
                    # extract the file from archive
                    output = subprocess.check_output(
                        [ar_path, "xN", str(current), archive_path, member],
                        cwd=objects_path,
                    ).decode()
                    assert not output
                    # rename the newly extracted file
                    output = subprocess.check_output(
                        ["mv", member, current_file], cwd=objects_path
                    ).decode()
                    assert not output
                    # rename the tmp file back to 'member'
                    output = subprocess.check_output(
                        ["mv", tmp_filename, member], cwd=objects_path
                    ).decode()
                    assert not output
                    known_objects.append(_gen_path(objects_path, current_file))

    elif file_type == ArchiveKind.THIN_ARCHIVE:
        output = subprocess.check_output([args.ar, "t", args.archive]).decode()
        for line in output.splitlines():
            assert os.path.exists(line)
            known_objects.append(line)
    elif file_type == ArchiveKind.UNKNOWN:
        raise AssertionError(
            f"unknown archive kind for file {args.archive}: {debug_output}"
        )

    manifest = {
        "debug": debug_output,
        "objects": known_objects,
    }
    with open(os.path.join(args.manifest_out), "w") as outfile:
        json.dump(manifest, outfile, indent=2, sort_keys=True)

    return 0


sys.exit(main(sys.argv))
