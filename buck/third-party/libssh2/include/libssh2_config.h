// NOTE (aseipp): this file is supposed to contain CMake-generated #define's in
// the upstream build system. we don't use CMake to build, and we pass those
// flags directly using -D, just like the libssh2-rs build.rs script does it. so
// this file only needs to exist so that the C source files can include it as a
// no-op
