# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

import argparse
import os
import stat
import sys
import zipfile


def _parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--src", required=True, help="File to extract")
    parser.add_argument("--dst", required=True, help="Output directory")

    return parser.parse_args()


def do_unzip(archive, output_dir):
    with zipfile.ZipFile(archive) as z:
        # First extract non-symlinks so that when symlinks are created in the next step
        # symlink type (whether it's a file or a directory which is important for Windows platform)
        # is automatically detected for non-broken symlinks (see documentation for `os.symlink` function).
        # That way we don't need to pass `target_is_directory` argument to `os.symlink` function.
        for info in (i for i in z.infolist() if not _is_symlink(i)):
            z.extract(info, path=output_dir)
        for info in (i for i in z.infolist() if _is_symlink(i)):
            symlink_path = os.path.join(output_dir, info.filename)
            symlink_dst = z.read(info).decode("utf-8")
            if os.path.isabs(symlink_dst):
                raise RuntimeError(
                    f"Symlink `{info.filename}` -> `{symlink_dst}` points to absolute path which is prohibited."
                )
            output_dir_relative_symlink_dst = os.path.normpath(
                os.path.join(os.path.dirname(info.filename), symlink_dst)
            )
            if output_dir_relative_symlink_dst.startswith(os.pardir):
                raise RuntimeError(
                    f"Symlink `{info.filename}` -> `{symlink_dst}` (normalized destination path relative to archive output directory is `{output_dir_relative_symlink_dst}`) points outside of archive output directory which is prohibited."
                )
            os.symlink(symlink_dst, symlink_path)


def _file_attributes(zip_info):
    # Those are stored in upper bits
    return zip_info.external_attr >> 16


def _is_symlink(zip_info):
    return stat.S_ISLNK(_file_attributes(zip_info))


def main():
    args = _parse_args()
    print("Source zip is: {}".format(args.src), file=sys.stderr)
    print("Output destination is: {}".format(args.dst), file=sys.stderr)
    do_unzip(args.src, args.dst)


if __name__ == "__main__":
    main()
