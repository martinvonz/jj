
load("//buck/shims/jj.bzl", "jj")
load("//cli/BUILD.deps.bzl", "JJ_CLI_DEPS")

alias(
    # NOTE: default target for this package
    name = 'cli',
    actual = ':jj',
)

jj.rust_library(
    name = "jj-cli",
    srcs = glob([
        "src/**/*.rs",
        "src/**/*.toml",
        "src/**/*.json",
        "src/**/*.pest",
    ], exclude = [
        "**/main.rs"
    ]),

    features = [
        "watchman",
        "git",
    ],

    deps = JJ_CLI_DEPS,
)

jj.rust_binary(
    name = 'jj',
    srcs = ['src/main.rs'],
    features = [
        "watchman",
        "git",
    ],
    deps = [ ":jj-cli" ] + JJ_CLI_DEPS,
)
