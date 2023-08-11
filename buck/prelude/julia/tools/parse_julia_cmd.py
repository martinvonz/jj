# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

import argparse
import json
import os
import pathlib
import subprocess
import sys
import tempfile


class jll_artifact:
    """Parses artifact info from json file and stores the relevant info."""

    def __init__(self, artifact_entry, json_path):
        self.artifact_name = artifact_entry[0]
        rel_path_to_json = pathlib.Path(*pathlib.Path(artifact_entry[1]).parts[1:])
        rel_path_of_binary = artifact_entry[2]
        self.artifacts_path = os.path.join(
            json_path, rel_path_to_json, rel_path_of_binary
        )

    def form_artifact_dependency(self):
        """Creates artifact dependency line that goes inside julia source file."""
        return '{} = "{}"'.format(self.artifact_name, self.artifacts_path)


class jll_library:
    """Parses the provided json file and stores the relevant info."""

    def __init__(self, json_entry, json_path):
        self.package_name = json_entry[0]
        self.uuid = json_entry[1]
        self.jll_artifacts = [jll_artifact(a, json_path) for a in json_entry[2]]

    def write_library(self, root_directory):
        """Creates and populates the library sources and directories"""
        self.package_dir = pathlib.Path(root_directory) / self.package_name
        self.package_dir.mkdir(parents=True, exist_ok=True)
        self._create_jll_src()
        self._create_project_toml()

    def _create_jll_src(self):
        """Creates the library src.jl file."""
        src_filename = self.package_name + ".jl"
        src_path = self.package_dir / "src"
        src_path.mkdir(parents=True, exist_ok=True)

        exports = [a.artifact_name for a in self.jll_artifacts]
        with open(src_path / src_filename, "w") as src_file:
            src_file.write("module " + self.package_name + "\n")
            src_file.write("export " + ", ".join(exports) + "\n")
            for a in self.jll_artifacts:
                src_file.write(a.form_artifact_dependency() + "\n")
            src_file.write("end\n")

    def _create_project_toml(self):
        """Creates the library Project.toml file."""
        with open(self.package_dir / "Project.toml", "w") as toml_file:
            toml_file.write('name = "{}"\n'.format(self.package_name))
            toml_file.write('uuid = "{}"\n'.format(self.uuid))
            toml_file.close()


def parse_json(args, lib_dir):
    """Pulls jll library data from json file and writes library files."""
    json_file = args.json_path
    json_path = os.path.split(json_file)[0]

    f = open(json_file)
    data = json.load(f)
    f.close()

    libs = []
    for entry in data:
        # parse the jll itself into our data structure
        jll_lib = jll_library(entry, json_path)
        # use that structure to "create" a new library in a temp directory
        jll_lib.write_library(lib_dir)
        # store for later if needed.
        libs.append(jll_lib)

    return libs


def parse_arg_string(arg_string):
    parsed = (arg_string.replace('"', "")).split(";;")
    # the first item is junk in order to trick python's argparse (*sigh*)
    if parsed:
        parsed = parsed[1:]
    return parsed


def build_command(args, lib_dir, depot_dir):
    """Builds the run command and env from the supplied args."""

    # Compose the environment variables to pass
    my_env = os.environ.copy()
    my_env["JULIA_LOAD_PATH"] = "{}:{}::".format(args.lib_path, lib_dir)
    my_env["JULIA_DEPOT_PATH"] = "{}:{}::".format(args.lib_path, depot_dir)

    # For now, we hard code the path of the shlibs relative to the json file.
    my_env["LD_LIBRARY_PATH"] = "{}:{}".format(
        pathlib.Path(args.lib_path) / "../__shared_libs_symlink_tree__",
        my_env.setdefault("LD_LIBRARY_PATH", ""),
    )

    # Iterate through environment variables passed by argparse
    if args.env != "":
        parsed_env = parse_arg_string(args.env)
        for varstring in parsed_env:
            var, value = varstring.split("=")
            my_env[var] = value

    # Compose main julia command
    my_command = [args.julia_binary]
    print(my_command)

    # Note that to properly pass a "list" of arguments via an argument, we
    # needed to join it with a delimiter (;;) and pass it as a string first.
    if args.julia_flags != "":
        my_command += parse_arg_string(args.julia_flags)

    my_command += [args.main]

    if args.main_args != "":
        my_command += parse_arg_string(args.main_args)

    return my_command, my_env


def main() -> int:
    """Sets up the julia environment with appropriate library aliases."""
    parser = argparse.ArgumentParser()
    parser.add_argument("--env", default="")
    parser.add_argument("--lib-path", default="")
    parser.add_argument("--json-path", default="")
    parser.add_argument("--julia-binary")
    parser.add_argument("--julia-flags", default="")
    parser.add_argument("--main")
    parser.add_argument("--main-args", default="")
    args = parser.parse_args()

    # create a temporary directory to store artifacts. Note that this temporary
    # directory will be deleted when the process exits.
    with tempfile.TemporaryDirectory() as lib_dir:
        with tempfile.TemporaryDirectory() as depot_dir:
            parse_json(args, lib_dir)
            my_command, my_env = build_command(args, lib_dir, depot_dir)
            code = subprocess.call(my_command, env=my_env)

    sys.exit(code)


if __name__ == "__main__":
    main()
