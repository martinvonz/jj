
load("//buck/shims/jj.bzl", "jj")

alias(
    # NOTE: default target for this package
    name = 'cli',
    actual = ':jj',
)

ALL_DEPS = [
    # CARGO-SYNC-START: dependencies
    '//lib:jj-lib',
    'third-party//rust:chrono',
    'third-party//rust:clap',
    'third-party//rust:clap-markdown',
    'third-party//rust:clap_complete',
    'third-party//rust:clap_complete_nushell',
    'third-party//rust:clap_mangen',
    'third-party//rust:config',
    'third-party//rust:criterion',
    'third-party//rust:crossterm',
    'third-party//rust:dirs',
    'third-party//rust:esl01-renderdag',
    'third-party//rust:futures',
    'third-party//rust:git2',
    'third-party//rust:gix',
    'third-party//rust:hex',
    'third-party//rust:indexmap',
    'third-party//rust:itertools',
    'third-party//rust:maplit',
    'third-party//rust:minus',
    'third-party//rust:once_cell',
    'third-party//rust:pest',
    'third-party//rust:pest_derive',
    'third-party//rust:pollster',
    'third-party//rust:rayon',
    'third-party//rust:regex',
    'third-party//rust:rpassword',
    'third-party//rust:scm-record',
    'third-party//rust:serde',
    'third-party//rust:slab',
    'third-party//rust:strsim',
    'third-party//rust:tempfile',
    'third-party//rust:textwrap',
    'third-party//rust:thiserror',
    'third-party//rust:timeago',
    'third-party//rust:toml_edit',
    'third-party//rust:tracing',
    'third-party//rust:tracing-chrome',
    'third-party//rust:tracing-subscriber',
    'third-party//rust:unicode-width',
    # CARGO-SYNC-END
] + select({
    "config//os:windows": [],
    "DEFAULT": [
        # CARGO-SYNC-START: dependencies@cfg(unix)
        'third-party//rust:libc',
        # CARGO-SYNC-END
    ],
})

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

    deps = ALL_DEPS,
)

jj.rust_binary(
    name = 'jj',
    srcs = ['src/main.rs'],
    features = [
        "watchman",
        "git",
    ],
    deps = [ ":jj-cli" ] + ALL_DEPS,
)
