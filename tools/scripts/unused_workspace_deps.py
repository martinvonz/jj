#!/usr/bin/env python3

# tools like cargo-udeps will only check if the dependencies listed in a
# crate's Cargo.toml file are actually used; it doesn't cross reference the
# workspace cargo file to find unused workspace deps. this script will do that

import tomllib

CRATE_TOML_FILES = [
    'cli/Cargo.toml',
    'lib/Cargo.toml',
    'lib/proc-macros/Cargo.toml',
    'lib/gen-protos/Cargo.toml',
    'lib/testutils/Cargo.toml',
]

def check_unused_deps():
    all_deps = None
    with open("Cargo.toml", "rb") as f:
        dat = tomllib.load(f)
        all_deps = dat["workspace"]["dependencies"]

    total_deps = len(all_deps)
    print(f"Found {total_deps} top-level dependencies in workspace Cargo.toml")

    # now, iterate over all the crate.toml files and check for unused dependencies
    # by deleting entries from all_deps, if they exist
    deleted_deps = 0
    for crate_toml in CRATE_TOML_FILES:
        with open(crate_toml, "rb") as f:
            dat = tomllib.load(f)
            deps = dat["dependencies"]

            if "build-dependencies" in dat:
                for x, v in dat["build-dependencies"].items():
                    deps[x] = v

            if "dev-dependencies" in dat:
                for x, v in dat["dev-dependencies"].items():
                    deps[x] = v

            if "target" in dat:
                for target in dat["target"]:
                    if target.startswith("cfg("):
                        for x, v in dat["target"][target]["dependencies"].items():
                            deps[x] = v

            for x in deps.keys():
                if x in all_deps:
                    del all_deps[x]
                    deleted_deps += 1

    print(f'Found {deleted_deps} unique dependencies among {len(CRATE_TOML_FILES)} Cargo.toml files')
    if len(all_deps) > 0:
        print(f"Found {len(all_deps)} unused dependencies:")
        for x in all_deps.keys():
            print(f"  {x}")

if __name__ == '__main__':
    check_unused_deps()
