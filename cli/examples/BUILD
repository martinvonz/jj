
load("//buck/shims/jj.bzl", "jj")
load("//cli/BUILD.deps.bzl", "JJ_CLI_DEPS")

EXAMPLES = [
    'custom-backend',
    'custom-command',
    'custom-commit-templater',
    'custom-global-flag',
    'custom-operation-templater',
    'custom-working-copy',
]

[
    jj.rust_binary(
        name = name,
        srcs = glob(["{}/**/*.rs".format(name)]),
        deps = [
            "//cli:jj-cli",
            "third-party//rust:async-trait",
        ] + JJ_CLI_DEPS,
    ) for name in EXAMPLES
]
