load("@prelude//rust:cargo_package.bzl", "cargo")
load("@prelude//rust:cargo_buildscript.bzl", "buildscript_run")

load(":jj.bzl", "jj")

def _rust_library(**_kwargs):
    cargo.rust_library(**_kwargs)

def _rust_binary(**_kwargs):
    cargo.rust_binary(**_kwargs)

def _cxx_library(**_kwargs):
    jj.cxx_library(**_kwargs)

def _prebuilt_cxx_library(**_kwargs):
    jj.prebuilt_cxx_library(**_kwargs)

third_party_rust = struct(
    rust_library = _rust_library,
    rust_binary = _rust_binary,
    cxx_library = _cxx_library,
    prebuilt_cxx_library = _prebuilt_cxx_library,
    buildscript_run = buildscript_run,
)
