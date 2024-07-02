
load("//buck/shims/jj.bzl", "jj")

[
    # set an alias for 'lib' which is the default for this package, and jj-lib
    # which other crates may set dependencies on. defaults to the full build
    alias(
        name = name,
        actual = ':jj-lib-full',
    ) for name in [ 'lib', 'jj-lib' ]
]

filegroup(
    name = 'protos.pb',
    srcs = glob(['src/protos/*.proto']),
    visibility = [ '//lib/...' ],
)

# extra dependencies for the full build of jj-lib
FULL_DEPS = [
    'third-party//rust:git2',
    'third-party//rust:gix',
    'third-party//rust:watchman_client',
]

# these deps are shared between all versions
# of jj-lib. filter out FULL_DEPS from this list
COMMON_DEPS = filter(lambda x: x not in FULL_DEPS, [
    # CARGO-SYNC-START: dependencies
    '//lib/proc-macros:jj-lib-proc-macros',
    'third-party//rust:async-trait',
    'third-party//rust:backoff',
    'third-party//rust:blake2',
    'third-party//rust:bytes',
    'third-party//rust:chrono',
    'third-party//rust:config',
    'third-party//rust:digest',
    'third-party//rust:either',
    'third-party//rust:futures',
    'third-party//rust:git2',
    'third-party//rust:gix',
    'third-party//rust:glob',
    'third-party//rust:hex',
    'third-party//rust:ignore',
    'third-party//rust:itertools',
    'third-party//rust:maplit',
    'third-party//rust:once_cell',
    'third-party//rust:pest',
    'third-party//rust:pest_derive',
    'third-party//rust:pollster',
    'third-party//rust:prost',
    'third-party//rust:rand',
    'third-party//rust:rand_chacha',
    'third-party//rust:rayon',
    'third-party//rust:ref-cast',
    'third-party//rust:regex',
    'third-party//rust:serde',
    'third-party//rust:serde_json',
    'third-party//rust:smallvec',
    'third-party//rust:strsim',
    'third-party//rust:tempfile',
    'third-party//rust:thiserror',
    'third-party//rust:tokio',
    'third-party//rust:tracing',
    'third-party//rust:watchman_client',
    'third-party//rust:whoami',
    'third-party//rust:zstd',
    # CARGO-SYNC-END
]) + select({
    "config//os:windows": [
        # CARGO-SYNC-START: dependencies@cfg(windows)
        'third-party//rust:winreg',
        # CARGO-SYNC-END
    ],
    "DEFAULT": [
        # CARGO-SYNC-START: dependencies@cfg(unix)
        'third-party//rust:rustix',
        # CARGO-SYNC-END
    ],
})

jj.rust_library(
    name = "jj-lib-full",
    crate = "jj_lib",
    srcs = glob([
        "src/**/*.pest",
        "src/**/*.rs",
    ], exclude = [
        "src/protos/*.rs"
    ]),
    mapped_srcs = {
        "//lib/gen-protos:protos.rs": "src/protos",
    },
    features = [
        "watchman",
        "git",
    ],
    deps = COMMON_DEPS + FULL_DEPS,
)

jj.rust_library(
    name = "jj-lib-core",
    crate = "jj_lib",
    srcs = glob([
        "src/**/*.pest",
        "src/**/*.rs",
    ], exclude = [
        "src/protos/*.rs"
    ]),
    mapped_srcs = {
        "//lib/gen-protos:protos.rs": "src/protos",
    },

    deps = COMMON_DEPS,
)
