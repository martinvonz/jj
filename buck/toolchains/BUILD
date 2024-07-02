load("@prelude//toolchains:rust.bzl", "system_rust_toolchain")
load("@prelude//toolchains:genrule.bzl", "system_genrule_toolchain")
load("@prelude//toolchains:cxx.bzl", "system_cxx_toolchain")

load(":defs.bzl",
    "simple_exe_download",
    "standalone_python_bootstrap_toolchain",
    "standalone_python_download",
)

# MARK: Rust Toolchain

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

toolchain_alias(
    name = 'cxx',
    actual = select({
        "mode//:debug": ":cxx-debug",
        "mode//:release": ":cxx-release",
    }),
)

# MARK: CXX Toolchain

cxx_warn_flags = select({
    "config//os:macos": [
        # NOTE (aseipp): some apparently ancient zlib code in the zlib-sys crate
        # needs this, otherwise builds fail on sufficiently recent macOS systems due
        # to C2X support officially making this an error by default
        "-Wno-implicit-function-declaration",
    ],
    "DEFAULT": [],
})

cxx_link_flags = select({
    "config//os:linux": [
        # on Linux, always use mold
        "-fuse-ld=mold"
    ],

    "config//os:macos": [
        # on macOS, use the new parallel linker
        "-fuse-ld=/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/ld",
        "-ld_new",
        "-framework",
        "Security",
    ],

    # on Windows, suffer
    "config//os:windows": [
        "winhttp.lib",
        "rpcrt4.lib",
        "ole32.lib",
        "crypt32.lib",
        "secur32.lib",
        "user32.lib",
    ],
})

debug_opt_flags = select({
    "config//os:windows": ["/Od"],
    "DEFAULT": ["-Og"],
})

release_opt_flags = select({
    "config//os:windows": ["/O2"],
    "DEFAULT": ["-O3"],
})

cxx_linker = select({
    "config//os:windows": "link.exe",
    "DEFAULT": "clang++",
})

system_cxx_toolchain(
    name = "cxx-debug",
    c_flags = debug_opt_flags + cxx_warn_flags,
    cxx_flags = debug_opt_flags + cxx_warn_flags,
    linker = cxx_linker,
    link_flags = cxx_link_flags,
)

system_cxx_toolchain(
    name = "cxx-release",
    c_flags = release_opt_flags + cxx_warn_flags,
    cxx_flags = release_opt_flags + cxx_warn_flags,
    linker = cxx_linker,
    link_flags = cxx_link_flags,
)

# MARK: Genrule Toolchain

system_genrule_toolchain(name = "genrule")

# MARK: Python (System) Toolchain

PYTHON_VERSION_TAG = '20240415'
PYTHON_VERSION = f'3.12.3+{PYTHON_VERSION_TAG}'

standalone_python_bootstrap_toolchain(
    name = "python_bootstrap",
    interpreter = select({
        'config//cpu:arm64': select({
            'config//os:linux': f'$(location :cpython-{PYTHON_VERSION}-aarch64-unknown-linux-gnu.tar.gz[python/bin/python])',
            'config//os:macos': f'$(location :cpython-{PYTHON_VERSION}-aarch64-apple-darwin.tar.gz[python/bin/python])',
        }),
        'config//cpu:x86_64': select({
            'config//os:linux':   f'$(location :cpython-{PYTHON_VERSION}-x86_64-unknown-linux-gnu.tar.gz[python/bin/python])',
            'config//os:windows': f'$(location :cpython-{PYTHON_VERSION}-x86_64-pc-windows-msvc.tar.gz[python/python.exe])',
        }),
    }),
)

[
    standalone_python_download(
        name = f'cpython-{PYTHON_VERSION}-{name}.tar.gz',
        sha256 = sha256,
        url = f'https://github.com/indygreg/python-build-standalone/releases/download/{PYTHON_VERSION_TAG}/cpython-{PYTHON_VERSION}-{name}-install_only.tar.gz',
        exe = bin,
        smoltar = select({
            'config//cpu:arm64': select({
                'config//os:linux': ':smoltar-aarch64-unknown-linux-musl',
                'config//os:macos': ':smoltar-aarch64-apple-darwin',
            }),
            'config//cpu:x86_64': select({
                'config//os:linux':   ':smoltar-x86_64-unknown-linux-musl',
                'config//os:windows': ':smoltar-x86_64-pc-windows-msvc.exe',
            }),
        }),
        visibility = [],
    ) for name, sha256, bin in [
        ('aarch64-unknown-linux-gnu', 'ec8126de97945e629cca9aedc80a29c4ae2992c9d69f2655e27ae73906ba187d', 'python/bin/python'),
        ('aarch64-apple-darwin', 'ccc40e5af329ef2af81350db2a88bbd6c17b56676e82d62048c15d548401519e', 'python/bin/python'),
        ('x86_64-unknown-linux-gnu', 'a73ba777b5d55ca89edef709e6b8521e3f3d4289581f174c8699adfb608d09d6', 'python/bin/python'),
        ('x86_64-pc-windows-msvc', 'f7cfa4ad072feb4578c8afca5ba9a54ad591d665a441dd0d63aa366edbe19279', 'python/python.exe'),
    ]
]

[
    simple_exe_download(
        name = name,
        sha256 = sha256,
        url = f'https://github.com/thoughtpolice/smoltar/releases/download/v0/{name}',
    ) for name, sha256 in [
        ('smoltar-aarch64-apple-darwin', 'd8964e7bdc8516130012302ea06d18cb130cee13e85145446f8ae473679fc7d4'),
        ('smoltar-aarch64-unknown-linux-musl', 'e1a0e2e4b102823c56fa96323d88fdef499ae855dca657d03582558cf48fc65f'),
        ('smoltar-x86_64-unknown-linux-musl', '3f727c5ada7a038f7e60afd003181779440fc11907cfece8a59c6be435ad3e67'),
        ('smoltar-x86_64-pc-windows-msvc.exe', '9b4ea3d1f3414574f16c23c2434d26e4f3cf67804db9d07d50ef3a00d7963184'),
    ]
]
