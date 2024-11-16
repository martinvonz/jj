
load("@root//buck/shims/jj.bzl", "jj")

load(
    ":BUILD.generated.bzl",
    "crypto_headers",
    "crypto_internal_headers",
    "crypto_sources",
    "crypto_sources_asm",
    "fips_fragments",
    "ssl_headers",
    "ssl_internal_headers",
    "ssl_sources",
    "tool_headers",
    "tool_sources",
)

VERSION = "2024.2.21+g386622719ab38a4f3a484a749bd6e1e6d5fda2e0"
GIT_COMMIT = VERSION.split('g', 1)[1]

http_archive(
    name = 'src',
    sha256 = 'c81ef84430272ce63f9308780981f84dae9a46179aa192f16f79370adf1d3a12',
    urls = [
        'https://github.com/google/boringssl/archive/{}.tar.gz'.format(GIT_COMMIT),
    ],
    type = 'tar.gz',
    strip_prefix = 'boringssl-{}'.format(GIT_COMMIT),
    sub_targets =
        crypto_headers + \
        crypto_internal_headers + \
        crypto_sources + \
        crypto_sources_asm + \
        fips_fragments + \
        ssl_headers + \
        ssl_internal_headers + \
        ssl_sources + \
        tool_headers + \
        tool_sources + [
            # rust sources
            'src/rust/bssl-sys/src/lib.rs',
            'src/rust/bssl-sys/rust_wrapper.c',
            'src/rust/bssl-sys/rust_wrapper.h',
            'src/rust/bssl-sys/wrapper.h',
        ],
    visibility = ['third-party//bssl/...'],
)

src_ref = lambda x: ':src[{}]'.format(x)

cc_copts = [
    # Assembler option --noexecstack adds .note.GNU-stack to each object to
    # ensure that binaries can be built with non-executable stack.
    "-Wa,--noexecstack",

    # This list of warnings should match those in the top-level CMakeLists.txt.
    "-Wall",
    "-Werror",
    "-Wformat=2",
    "-Wsign-compare",
    "-Wmissing-field-initializers",
    "-Wwrite-strings",
    "-Wshadow",
    "-fno-common",
]
cc_copts_c11 = [
    "-std=c11",
    "-Wmissing-prototypes",
    "-Wold-style-definition",
    "-Wstrict-prototypes",
]
cc_copts_cxx = [
    "-std=c++14",
    "-Wmissing-declarations",
]

boringssl_copts = [
    "-DBORINGSSL_IMPLEMENTATION",
] + select({
    # We assume that non-Windows builds use a GCC-compatible toolchain and that
    # Windows builds do not.
    "config//os:windows": [],
    "DEFAULT": cc_copts,
}) + select({
    # This is needed on glibc systems to get rwlock in pthreads, but it should
    # not be set on Apple platforms or FreeBSD, where it instead disables APIs
    # we use.
    # See compat(5), sys/cdefs.h, and https://crbug.com/boringssl/471
    "config//os:linux": ["-D_XOPEN_SOURCE=700"],
    # Without WIN32_LEAN_AND_MEAN, <windows.h> pulls in wincrypt.h, which
    # conflicts with our <openssl/x509.h>.
    "config//os:windows": ["-DWIN32_LEAN_AND_MEAN", "-DNOMINMAX"],
    "DEFAULT": [],
}) + select({
    "config//os:windows": ["-DOPENSSL_NO_ASM"],
    "DEFAULT": [],
})

boringssl_copts_c11 = boringssl_copts + select({
    "config//os:windows": ["/std:c11"],
    "DEFAULT": cc_copts_c11,
})

boringssl_copts_cxx = boringssl_copts + select({
    "config//os:windows": [],
    "DEFAULT": cc_copts_cxx,
})

jj.cxx_library(
    name = "crypto",
    srcs = map(src_ref, crypto_sources) + select({
        "config//os:windows": [],
        "DEFAULT": map(src_ref, crypto_sources_asm),
    }),
    compiler_flags = boringssl_copts_c11,

    header_namespace = "",
    exported_headers = {
        x: y for (x, y) in map(lambda x: (x[12:], src_ref(x)), crypto_headers)
    },

    preferred_linkage = "static",
    linker_flags = select({
        "config//os:windows": ["-defaultlib:advapi32.lib"],
        "DEFAULT": ["-pthread"],
    }),
)

jj.cxx_library(
    name = "ssl",
    srcs = map(src_ref, ssl_sources),
    compiler_flags = boringssl_copts_cxx,

    header_namespace = "",
    exported_headers = {
        x: y for (x, y) in map(lambda x: (x[12:], src_ref(x)), ssl_headers)
    },

    preferred_linkage = "static",
    exported_linker_flags = select({
        'config//os:linux': [ '-lstdc++' ],
        'config//os:macos': [ '-lc++' ],
        'DEFAULT': []
    }),
    deps = [ ":crypto" ],
)

jj.cxx_binary(
    name = "bssl",
    srcs = map(src_ref, tool_sources),
    compiler_flags = boringssl_copts_cxx,
    deps = [ ":crypto", ":ssl" ],
    visibility = ['PUBLIC'],
)
