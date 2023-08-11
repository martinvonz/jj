#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# Translated from the Rust `cc` crate's windows_registry.rs.
# https://github.com/rust-lang/cc-rs/blob/1.0.79/src/windows_registry.rs

import argparse
import json
import os
import shutil
import subprocess
import sys
import winreg
from pathlib import Path
from typing import IO, List, NamedTuple


class OutputJsonFiles(NamedTuple):
    # We write a Tool instance as JSON into each of these files.
    cl: IO[str]
    lib: IO[str]
    ml64: IO[str]
    link: IO[str]


class Tool(NamedTuple):
    exe: Path
    LIB: List[Path] = []
    PATH: List[Path] = []
    INCLUDE: List[Path] = []


def find_in_path(executable):
    which = shutil.which(executable)
    if which is None:
        print(f"{executable} not found in $PATH", file=sys.stderr)
        sys.exit(1)
    return Tool(which)


def find_with_vswhere_exe():
    program_files = os.environ.get("ProgramFiles(x86)")
    if program_files is None:
        program_files = os.environ.get("ProgramFiles")
    if program_files is None:
        print(
            "expected a %ProgramFiles(x86)% or %ProgramFiles% environment variable",
            file=sys.stderr,
        )
        sys.exit(1)

    vswhere_exe = (
        Path(program_files) / "Microsoft Visual Studio" / "Installer" / "vswhere.exe"
    )
    vswhere_json = subprocess.check_output(
        [
            vswhere_exe,
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-format",
            "json",
            "-nologo",
        ],
        encoding="utf-8",
    )

    vswhere_json = json.loads(vswhere_json)

    # Sort by MSVC version, newest to oldest.
    # Version is a sequence of 16-bit integers.
    # Example: "17.6.33829.357"
    vswhere_json.sort(
        key=lambda vs: [int(n) for n in vs["installationVersion"].split(".")],
        reverse=True,
    )

    for vs_instance in list(vswhere_json):
        installation_path = Path(vs_instance["installationPath"])

        # Tools version is different from the one above: "14.36.32532"
        version_file = (
            installation_path
            / "VC"
            / "Auxiliary"
            / "Build"
            / "Microsoft.VCToolsVersion.default.txt"
        )
        vc_tools_version = version_file.read_text(encoding="utf-8").strip()

        tools_path = installation_path / "VC" / "Tools" / "MSVC" / vc_tools_version
        bin_path = tools_path / "bin" / "HostX64" / "x64"
        lib_path = tools_path / "lib" / "x64"
        include_path = tools_path / "include"

        exe_names = "cl.exe", "lib.exe", "ml64.exe", "link.exe"
        if not all(bin_path.joinpath(exe).exists() for exe in exe_names):
            continue

        PATH = [bin_path]
        LIB = [lib_path]
        INCLUDE = [include_path]

        ucrt, ucrt_version = get_ucrt_dir()
        if ucrt and ucrt_version:
            PATH.append(ucrt / "bin" / ucrt_version / "x64")
            LIB.append(ucrt / "lib" / ucrt_version / "ucrt" / "x64")
            INCLUDE.append(ucrt / "include" / ucrt_version / "ucrt")

        sdk, sdk_version = get_sdk10_dir()
        if sdk and sdk_version:
            PATH.append(sdk / "bin" / "x64")
            LIB.append(sdk / "lib" / sdk_version / "um" / "x64")
            INCLUDE.append(sdk / "include" / sdk_version / "um")
            INCLUDE.append(sdk / "include" / sdk_version / "cppwinrt")
            INCLUDE.append(sdk / "include" / sdk_version / "winrt")
            INCLUDE.append(sdk / "include" / sdk_version / "shared")

        return [
            Tool(exe=bin_path / exe, LIB=LIB, PATH=PATH, INCLUDE=INCLUDE)
            for exe in exe_names
        ]

    print(
        "vswhere.exe did not find a suitable MSVC toolchain containing cl.exe, lib.exe, ml64.exe",
        file=sys.stderr,
    )
    sys.exit(1)


# To find the Universal CRT we look in a specific registry key for where all the
# Universal CRTs are located and then sort them asciibetically to find the
# newest version. While this sort of sorting isn't ideal, it is what vcvars does
# so that's good enough for us.
#
# Returns a pair of (root, version) for the ucrt dir if found.
def get_ucrt_dir():
    registry = winreg.ConnectRegistry(None, winreg.HKEY_LOCAL_MACHINE)
    key_name = "SOFTWARE\\Microsoft\\Windows Kits\\Installed Roots"
    registry_key = winreg.OpenKey(registry, key_name)
    kits_root = Path(winreg.QueryValueEx(registry_key, "KitsRoot10")[0])

    available_versions = [
        entry.name
        for entry in kits_root.joinpath("lib").iterdir()
        if entry.name.startswith("10.") and entry.joinpath("ucrt").is_dir()
    ]

    max_version = max(available_versions) if available_versions else None
    return kits_root, max_version


# Vcvars finds the correct version of the Windows 10 SDK by looking for the
# include `um\Windows.h` because sometimes a given version will only have UCRT
# bits without the rest of the SDK. Since we only care about libraries and not
# includes, we instead look for `um\x64\kernel32.lib`. Since the 32-bit and
# 64-bit libraries are always installed together we only need to bother checking
# x64, making this code a tiny bit simpler. Like we do for the Universal CRT, we
# sort the possibilities asciibetically to find the newest one as that is what
# vcvars does. Before doing that, we check the "WindowsSdkDir" and
# "WindowsSDKVersion" environment variables set by vcvars to use the environment
# sdk version if one is already configured.
#
# Returns a pair of (root, version).
def get_sdk10_dir():
    windows_sdk_dir = os.environ.get("WindowsSdkDir")
    windows_sdk_version = os.environ.get("WindowsSDKVersion")
    if windows_sdk_dir is not None and windows_sdk_version is not None:
        return windows_sdk_dir, windows_sdk_version.removesuffix("\\")

    registry = winreg.ConnectRegistry(None, winreg.HKEY_LOCAL_MACHINE)
    key_name = "SOFTWARE\\Microsoft\\Microsoft SDKs\\Windows\\v10.0"
    registry_key = winreg.OpenKey(
        registry, key_name, access=winreg.KEY_READ | winreg.KEY_WOW64_32KEY
    )
    installation_folder = Path(
        winreg.QueryValueEx(registry_key, "InstallationFolder")[0]
    )

    available_versions = [
        entry.name
        for entry in installation_folder.joinpath("lib").iterdir()
        if entry.joinpath("um", "x64", "kernel32.lib").is_file()
    ]

    max_version = max(available_versions) if available_versions else None
    return installation_folder, max_version


def write_tool_json(out, tool):
    j = json.dumps(
        tool._asdict(),
        indent=4,
        default=lambda path: str(path),
    )
    out.write(j)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cl", type=argparse.FileType("w"), required=True)
    parser.add_argument("--lib", type=argparse.FileType("w"), required=True)
    parser.add_argument("--ml64", type=argparse.FileType("w"), required=True)
    parser.add_argument("--link", type=argparse.FileType("w"), required=True)
    output = OutputJsonFiles(**vars(parser.parse_args()))

    # If vcvars has been run, it puts these tools onto $PATH.
    if "VCINSTALLDIR" in os.environ:
        cl_exe = find_in_path("cl.exe")
        lib_exe = find_in_path("lib.exe")
        ml64_exe = find_in_path("ml64.exe")
        link_exe = find_in_path("link.exe")
    else:
        cl_exe, lib_exe, ml64_exe, link_exe = find_with_vswhere_exe()

    write_tool_json(output.cl, cl_exe)
    write_tool_json(output.lib, lib_exe)
    write_tool_json(output.ml64, ml64_exe)
    write_tool_json(output.link, link_exe)


if __name__ == "__main__":
    main()
