
load(":defs.bzl", "vendored_rust_toolchain", "download_rust_toolchain")

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

DEFAULT_RUST_TOOLCHAIN = 'rustc-nightly-2024-08-08'

vendored_rust_toolchain(
    name = "rust-debug",
    toolchain = f':{DEFAULT_RUST_TOOLCHAIN}',
    default_edition = "2021",
    rustc_flags = common_rustc_flags,
)

vendored_rust_toolchain(
    name = "rust-release",
    toolchain = f':{DEFAULT_RUST_TOOLCHAIN}',
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

download_rust_toolchain(
    name = 'nightly-2024-08-08',
    channel = 'nightly',
    version = '2024-08-08',
    hashes = [
        ('aarch64-unknown-linux-gnu', 'b1693ef2e0aa090f29f39c9e84e85ed361ee80ab56aaf208520bc56f44f72900'),
        ('aarch64-apple-darwin', 'a0c3680b55065ddb488f208156d8d72b42d746926e73959c71f32dbd8561dc83'),
        ('x86_64-unknown-linux-gnu', 'ae9c934fe34718f9e148f400399f1f5ec350bddbb7e6e0fbe496d468de34f425'),
        ('x86_64-pc-windows-msvc', '9d2e7f7d3c7b71538b57987d816d41c21f0ee672d546b39a71f4385a2ce21327'),
    ]
)

download_rust_toolchain(
    name = 'stable-1.80.1',
    channel = 'stable',
    version = '1.80.1',
    hashes = [
        ('aarch64-unknown-linux-gnu', '4ae791da82be6e7223f4f6cd477799624481aa1011e17c37753a57d257f89198'),
        ('aarch64-apple-darwin', '170ea11a424d67bbf16df3a4788e0d844ced00490e44c18c366304db1ef8ca6d'),
        ('x86_64-unknown-linux-gnu', '9eedaea9719914a0f4673899aa11693607f25486569913fcca5905ef1da784ec'),
        ('x86_64-pc-windows-msvc', '027faf943cb6f413492c3abd8024b3608a93bffdc542371a219585085373c815'),
    ]
)
