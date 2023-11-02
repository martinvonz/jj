use std::env;

#[allow(dead_code)]
fn new_cc_builder() -> (String, bool) {
    let opt_level = env::var("OPT_LEVEL").unwrap_or_else(|_| String::from("1"));
    let is_debug = opt_level == "0";

    // NOTE (aseipp): on Linux, many distros enable hardening options like
    // -D_FORTIFY_SOURCE=2 in their C toolchains by default, and so if compiling
    // with -O0, the standard header libraries throw ugly warnings about how
    // no-optimization causes _FORTIFY_SOURCE to not work.
    //
    // instead, just detect when the Rust toolchain sets OPT_LEVEL=0 (default
    // debug/test profile), and then set it to debug instead.
    let c_opt_level = if is_debug { "g" } else { opt_level.as_str() };

    (c_opt_level.to_owned(), is_debug)
}

#[allow(dead_code)]
fn build_mimalloc() {
    let (c_opt_level, is_debug) = new_cc_builder();
    cc::Build::new()
        .include("cbits/mimalloc/include")
        .include("cbits/mimalloc/src")
        // Use the (convenient) amalgamation.
        .file("cbits/mimalloc/src/static.c")
        // In some configurations, there may be unused parameters. Suppress
        // them in cargo build output
        .flag_if_supported("-Wno-unused-parameter")
        // MI_SECURE implies four levels of hardening:
        //
        // - MI_SECURE=1: guard page around metadata
        // - MI_SECURE=2: guard page around each mimalloc page
        // - MI_SECURE=3: encoded freelists (corrupt freelist, invalid free)
        // - MI_SECURE=4: double free (may be more expensive)
        //
        // by default, against glibc 2.38, MI_SECURE=0 gives anywhere from 5-18%
        // performance uplift for free. MI_SECURE=1 gives about a 7% uplift,
        // while 2,3,4 all basically yield ~zero meaningful uplift in the ~1%
        // range, i.e. even the most secure version of mimalloc is still
        // baseline competitive.
        //
        // for jj, in non-debug configs, we set MI_SECURE=0. the goal is to
        // inevitably banish most C code to the shadow realm, and to the extent
        // we can't, it should be located here. since most Rust code will be
        // both int and buffer overflow-safe, that mitigates some of the need
        // for features like UAF and guard pages. on the other hand, we enable
        // those features in debug builds, since rust performance already takes
        // a massive hit in debug mode; with nextest these options have ~zero
        // visible impact on the test suite, it seems. and we want to make sure
        // we can catch as many bugs as possible.
        .define("MI_SECURE", if is_debug { "4" } else { "0" })
        // Disable debug assertions.
        .define("MI_DEBUG", if is_debug { "1" } else { "0" })
        // Enable statistics tracking. 0 is disabled, 1 is enabled, and 2 is
        // detailed info. 2 causes a ~5% performance hit.
        .define("MI_STAT", "2")
        // Enable the proper optimization level for the library
        .opt_level_str(&c_opt_level)
        // Build
        .compile("jj-mimalloc");
}

fn main() {
    #[cfg(feature = "mimalloc")]
    build_mimalloc();
}
