
load("//buck/shims/jj.bzl", "jj")

http_archive(
    name = 'protoc-bin-vendored.tar.gz',
    sha256 = '08f5cc8f522af7ba8f9f1a81f93bba94cef9312e5e644df4ecc08ea7ce9c40f7',
    urls = [
        'https://github.com/stepancheg/rust-protoc-bin-vendored/archive/20ebfb74aad5fb0f084ca2c2ebc0885b4ad18430.tar.gz',
    ],
    type = 'tar.gz',
    strip_prefix = 'rust-protoc-bin-vendored-20ebfb74aad5fb0f084ca2c2ebc0885b4ad18430',
    sub_targets = [
        'protoc-bin-vendored-linux-aarch_64/bin/protoc',
        'protoc-bin-vendored-linux-aarch_64/include',
        'protoc-bin-vendored-linux-x86_64/bin/protoc',
        'protoc-bin-vendored-linux-x86_64/include',
        'protoc-bin-vendored-macos-x86_64/bin/protoc',
        'protoc-bin-vendored-macos-x86_64/include',
        'protoc-bin-vendored-win32/bin/protoc.exe',
        'protoc-bin-vendored-win32/include',
    ],
)

jj.rust_binary(
    name = 'gen-protos',
    srcs = ['src/main.rs'],
    env = {
        'CARGO_MANIFEST_DIR': '$(location //lib:protos.pb)',
    },
    deps = [
        # CARGO-SYNC-START: dependencies
        'third-party//rust:prost-build',
        # CARGO-SYNC-END
    ]
)

genrule(
    name = 'protos.rs',
    cmd = "$(exe :gen-protos)",
    out = ".",
    env = {
        'PROTOC': select({
            'config//cpu:arm64': select({
                'config//os:linux': '$(location :protoc-bin-vendored.tar.gz[protoc-bin-vendored-linux-aarch_64/bin/protoc])',
                'config//os:macos': '$(location :protoc-bin-vendored.tar.gz[protoc-bin-vendored-macos-x86_64/bin/protoc])',
            }),
            'config//cpu:x86_64': select({
                'config//os:linux': '$(location :protoc-bin-vendored.tar.gz[protoc-bin-vendored-linux-x86_64/bin/protoc])',
                'config//os:windows': '$(location :protoc-bin-vendored.tar.gz[protoc-bin-vendored-win32/bin/protoc.exe])',
            })
        }),
        'PROTOC_INCLUDE': select({
            'config//cpu:arm64': select({
                'config//os:linux': '$(location :protoc-bin-vendored.tar.gz[protoc-bin-vendored-linux-aarch_64/include])',
                'config//os:macos': '$(location :protoc-bin-vendored.tar.gz[protoc-bin-vendored-macos-x86_64/include])',
            }),
            'config//cpu:x86_64': select({
                'config//os:linux': '$(location :protoc-bin-vendored.tar.gz[protoc-bin-vendored-linux-x86_64/include])',
                'config//os:windows': '$(location :protoc-bin-vendored.tar.gz[protoc-bin-vendored-win32/include])',
            })
        })
    }
)
