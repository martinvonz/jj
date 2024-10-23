
load("@root//buck/shims/jj.bzl", "jj")

VERSION = '1.3.1'

http_archive(
    name = 'src',
    sha256 = '9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23',
    urls = [
        f'https://github.com/madler/zlib/releases/download/v{VERSION}/zlib-{VERSION}.tar.gz',
    ],
    type = 'tar.gz',
    strip_prefix = f'zlib-{VERSION}',
    sub_targets = [
        "adler32.c",
        "compress.c",
        "crc32.c",
        "deflate.c",
        "infback.c",
        "inffast.c",
        "inflate.c",
        "inftrees.c",
        "trees.c",
        "uncompr.c",
        "zutil.c",
        "gzclose.c",
        "gzlib.c",
        "gzread.c",
        "gzwrite.c",
        "zlib.h",
        "zconf.h",
    ],
)

jj.cxx_library(
    name = 'libz',
    srcs = [
        ':src[adler32.c]',
        ':src[compress.c]',
        ':src[crc32.c]',
        ':src[deflate.c]',
        ':src[infback.c]',
        ':src[inffast.c]',
        ':src[inflate.c]',
        ':src[inftrees.c]',
        ':src[trees.c]',
        ':src[uncompr.c]',
        ':src[zutil.c]',
        ':src[gzclose.c]',
        ':src[gzlib.c]',
        ':src[gzread.c]',
        ':src[gzwrite.c]',
    ],
    preferred_linkage = "static",
    headers = [],
    header_namespace = "",
    exported_headers = {
        'zconf.h': ':src[zconf.h]',
        'zlib.h': ':src[zlib.h]',
    },
    compiler_flags = [
        "-DSTDC",
        "-D_LARGEFILE64_SOURCE",
        "-D_POSIX_SOURCE",
    ],
)
