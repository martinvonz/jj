
load("@prelude//toolchains:rust.bzl", "system_rust_toolchain")

toolchain_alias(
    name = 'rust',
    actual = select({
        "mode//:debug": ":rust-debug",
        "mode//:release": ":rust-release",
    }),
)

common_rustc_flags = select({
    'config//os:windows': [
        # NOTE: on Windows, link the CRT statically so that users don't need
        # a copy of vcruntime140.dll on their machines. its distribution is...
        # complex
        "-Ctarget-feature=+crt-static",
    ],
    'DEFAULT': [],
})

system_rust_toolchain(
    name = "rust-debug",
    default_edition = "2021",
    rustc_flags = common_rustc_flags,
)

system_rust_toolchain(
    name = "rust-release",
    default_edition = "2021",
    rustc_flags = common_rustc_flags + [
        "-Copt-level=3",
        "-Cdebuginfo=none",
        "-Cdebug-assertions=false",
        "-Coverflow-checks=false",
        "-Clto=false",
        "-Ccodegen-units=1",
    ],
)
