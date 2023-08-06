load("@prelude//utils/buckconfig.bzl", "read_choice")
load(":defs.bzl", "generate_platforms", "default_platforms")

build_config = read_choice("jj", "buildmode", [
    "debug",
    "release",
], "debug")

default_constraints = [
    # build mode: debug or release
    ('mode//:{}'.format(build_config))
]

generate_platforms(default_platforms, default_constraints)
