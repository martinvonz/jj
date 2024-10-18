
load("//buck/shims/jj.bzl", "jj")

alias(
    # NOTE: default target for this package
    name = 'proc-macros',
    actual = ':jj-lib-proc-macros',
)

jj.rust_library(
    name = 'jj-lib-proc-macros',
    srcs = glob(["src/**/*.rs"]),
    proc_macro = True,
    deps = [
        # CARGO-SYNC-START: dependencies
        'third-party//rust:proc-macro2',
        'third-party//rust:quote',
        'third-party//rust:syn',
        # CARGO-SYNC-END
    ],
    visibility = ['PUBLIC'], # XXX FIXME (aseipp): why is this needed?
)
