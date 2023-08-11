# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# Update with
#   python3 -c 'import json, requests; print(json.dumps(requests.get("https://ziglang.org/download/index.json").json(), indent=4))'
releases = {
    "0.1.1": {
        "date": "2017-10-17",
        "docs": "https://ziglang.org/documentation/0.1.1/",
        "notes": "https://ziglang.org/download/0.1.1/release-notes.html",
        "src": {
            "shasum": "ffca0cfb263485287e19cc997b08701fcd5f24b700345bcdc3dd8074f5a104e0",
            "size": "1659716",
            "tarball": "https://ziglang.org/download/0.1.1/zig-0.1.1.tar.xz",
        },
        "x86_64-windows": {
            "shasum": "6fc88bef531af7e567fe30bf60da1487b86833cbee84c7a2f3e317030aa5b660",
            "size": "19757776",
            "tarball": "https://ziglang.org/download/0.1.1/zig-win64-0.1.1.zip",
        },
    },
    "0.2.0": {
        "date": "2018-03-15",
        "docs": "https://ziglang.org/documentation/0.2.0/",
        "notes": "https://ziglang.org/download/0.2.0/release-notes.html",
        "src": {
            "shasum": "29c9beb172737f4d5019b88ceae829ae8bc6512fb4386cfbf895ae2b42aa6965",
            "size": "1940832",
            "tarball": "https://ziglang.org/download/0.2.0/zig-0.2.0.tar.xz",
        },
        "x86_64-linux": {
            "shasum": "209c6fb745d42474c0a73d6f291c7ae3a38b6a1b6b641eea285a7f840cc1a890",
            "size": "22551928",
            "tarball": "https://ziglang.org/download/0.2.0/zig-linux-x86_64-0.2.0.tar.xz",
        },
        "x86_64-windows": {
            "shasum": "4f8a2979941a1f081ec8e545cca0b72608c0db1c5a3fd377a94db40649dcd3d4",
            "size": "21076274",
            "tarball": "https://ziglang.org/download/0.2.0/zig-win64-0.2.0.zip",
        },
    },
    "0.3.0": {
        "date": "2018-09-28",
        "docs": "https://ziglang.org/documentation/0.3.0/",
        "notes": "https://ziglang.org/download/0.3.0/release-notes.html",
        "src": {
            "shasum": "d70af604f3a8622f3393d93abb3e056bf60351e32d121e6fa4fe03d8d41e1f5a",
            "size": "2335592",
            "tarball": "https://ziglang.org/download/0.3.0/zig-0.3.0.tar.xz",
        },
        "x86_64-linux": {
            "shasum": "b378d0aae30cb54f28494e7bc4efbc9bfb6326f47bfb302e8b5287af777b2f3c",
            "size": "25209304",
            "tarball": "https://ziglang.org/download/0.3.0/zig-linux-x86_64-0.3.0.tar.xz",
        },
        "x86_64-macos": {
            "shasum": "19dec1f1943ab7be26823376d466f7e456143deb34e17502778a949034dc2e7e",
            "size": "23712696",
            "tarball": "https://ziglang.org/download/0.3.0/zig-macos-x86_64-0.3.0.tar.xz",
        },
        "x86_64-windows": {
            "shasum": "bb568c03950958f8bb3472139c3ab5ed74547c8c694ab50f404c202faf51baf4",
            "size": "22524425",
            "tarball": "https://ziglang.org/download/0.3.0/zig-windows-x86_64-0.3.0.zip",
        },
    },
    "0.4.0": {
        "date": "2019-04-08",
        "docs": "https://ziglang.org/documentation/0.4.0/",
        "notes": "https://ziglang.org/download/0.4.0/release-notes.html",
        "src": {
            "shasum": "fec1f3f6b359a3d942e0a7f9157b3b30cde83927627a0e1ea95c54de3c526cfc",
            "size": "5348776",
            "tarball": "https://ziglang.org/download/0.4.0/zig-0.4.0.tar.xz",
        },
        "x86_64-freebsd": {
            "shasum": "3d557c91ac36d8262eb1733bb5f261c95944f9b635e43386e3d00a3272818c30",
            "size": "27269672",
            "tarball": "https://ziglang.org/download/0.4.0/zig-freebsd-x86_64-0.4.0.tar.xz",
        },
        "x86_64-linux": {
            "shasum": "fb1954e2fb556a01f8079a08130e88f70084e08978ff853bb2b1986d8c39d84e",
            "size": "32876100",
            "tarball": "https://ziglang.org/download/0.4.0/zig-linux-x86_64-0.4.0.tar.xz",
        },
        "x86_64-macos": {
            "shasum": "67c932982484d017c5111e54af9f33f15e8e05c6bc5346a55e04052159c964a8",
            "size": "30841504",
            "tarball": "https://ziglang.org/download/0.4.0/zig-macos-x86_64-0.4.0.tar.xz",
        },
        "x86_64-windows": {
            "shasum": "fbc3dd205e064c263063f69f600bedb18e3d0aa2efa747a63ef6cafb6d73f127",
            "size": "35800101",
            "tarball": "https://ziglang.org/download/0.4.0/zig-windows-x86_64-0.4.0.zip",
        },
    },
    "0.5.0": {
        "date": "2019-09-30",
        "docs": "https://ziglang.org/documentation/0.5.0/",
        "notes": "https://ziglang.org/download/0.5.0/release-notes.html",
        "src": {
            "shasum": "55ae16960f152bcb9cf98b4f8570902d0e559a141abf927f0d3555b7cc838a31",
            "size": "10956132",
            "tarball": "https://ziglang.org/download/0.5.0/zig-0.5.0.tar.xz",
        },
        "x86_64-freebsd": {
            "shasum": "9e1f4d36c3d584c0aa01f20eb4cd0a0eef3eee5af23e483b8414de55feab6ab6",
            "size": "33650744",
            "tarball": "https://ziglang.org/download/0.5.0/zig-freebsd-x86_64-0.5.0.tar.xz",
        },
        "x86_64-linux": {
            "shasum": "43e8f8a8b8556edd373ddf9c1ef3ca6cf852d4d09fe07d5736d12fefedd2b4f7",
            "size": "40895068",
            "tarball": "https://ziglang.org/download/0.5.0/zig-linux-x86_64-0.5.0.tar.xz",
        },
        "x86_64-macos": {
            "shasum": "28702cc05745c7c0bd450487d5f4091bf0a1ad279b35eb9a640ce3e3a15b300d",
            "size": "37898664",
            "tarball": "https://ziglang.org/download/0.5.0/zig-macos-x86_64-0.5.0.tar.xz",
        },
        "x86_64-windows": {
            "shasum": "58141323db8d84a5af62746be5f9140bc161ee760ef33dc91a887bf9ac021976",
            "size": "44871804",
            "tarball": "https://ziglang.org/download/0.5.0/zig-windows-x86_64-0.5.0.zip",
        },
    },
    "0.6.0": {
        "aarch64-linux": {
            "shasum": "e7520efd42cfa02be48c2e430d08fe1f3cbb999d21d9f0d3ffd0febb976b2f41",
            "size": "37090044",
            "tarball": "https://ziglang.org/download/0.6.0/zig-linux-aarch64-0.6.0.tar.xz",
        },
        "armv6kz-linux": {
            "shasum": "36b6493b3fed43eb1f0000e765798ad31a6bb7d7fd3f553ac1c3761dbc919b82",
            "size": "39133452",
            "tarball": "https://ziglang.org/download/0.6.0/zig-linux-armv6kz-0.6.0.tar.xz",
        },
        "armv7a-linux": {
            "shasum": "946969abe357def95ca9cbbfcebfcf2d90cf967bcd3f48ee87662e32d91d8f35",
            "size": "39143748",
            "tarball": "https://ziglang.org/download/0.6.0/zig-linux-armv7a-0.6.0.tar.xz",
        },
        "bootstrap": {
            "shasum": "5e0e4dc878b3dd0c1852a442b174f0732e8c07869a8fcd226b71a93b89b381ab",
            "size": "38469948",
            "tarball": "https://ziglang.org/download/0.6.0/zig-bootstrap-0.6.0.tar.xz",
        },
        "date": "2020-04-13",
        "docs": "https://ziglang.org/documentation/0.6.0/",
        "i386-linux": {
            "shasum": "a97a2f9ae21575743cdd763c1917d49400d83fc562ef64582b18bade43eb24ce",
            "size": "44877640",
            "tarball": "https://ziglang.org/download/0.6.0/zig-linux-i386-0.6.0.tar.xz",
        },
        "i386-windows": {
            "shasum": "3b0a02618743e92175990dc6d1a787bb95ff62c4cda016f1c14c7786f575f8ca",
            "size": "60446431",
            "tarball": "https://ziglang.org/download/0.6.0/zig-windows-i386-0.6.0.zip",
        },
        "notes": "https://ziglang.org/download/0.6.0/release-notes.html",
        "riscv64-linux": {
            "shasum": "68ddee43f7503c8ae5f26a921f3602c34719a02ed2241f528c0b8b888cc14b38",
            "size": "41993144",
            "tarball": "https://ziglang.org/download/0.6.0/zig-linux-riscv64-0.6.0.tar.xz",
        },
        "src": {
            "shasum": "5d167dc19354282dd35dd17b38e99e1763713b9be8a4ba9e9e69284e059e7204",
            "size": "10349552",
            "tarball": "https://ziglang.org/download/0.6.0/zig-0.6.0.tar.xz",
        },
        "stdDocs": "https://ziglang.org/documentation/0.6.0/std/",
        "x86_64-freebsd": {
            "shasum": "190ff79c1eb56805a315d7c7a51082e32f62926250c0702b36760c225e1634a3",
            "size": "36974604",
            "tarball": "https://ziglang.org/download/0.6.0/zig-freebsd-x86_64-0.6.0.tar.xz",
        },
        "x86_64-linux": {
            "shasum": "08fd3c757963630645441c2772362e9c2294020c44f14fce1b89f45de0dc1253",
            "size": "44766320",
            "tarball": "https://ziglang.org/download/0.6.0/zig-linux-x86_64-0.6.0.tar.xz",
        },
        "x86_64-macos": {
            "shasum": "17270360e87ddc49f737e760047b2fac49f1570a824a306119b1194ac4093895",
            "size": "42573184",
            "tarball": "https://ziglang.org/download/0.6.0/zig-macos-x86_64-0.6.0.tar.xz",
        },
        "x86_64-windows": {
            "shasum": "c3b897832523e1026e10b2d8d55d7f895185c0a27a63681f3a23219c3f1c38f4",
            "size": "49065511",
            "tarball": "https://ziglang.org/download/0.6.0/zig-windows-x86_64-0.6.0.zip",
        },
    },
    "0.7.0": {
        "aarch64-linux": {
            "shasum": "f89933bac87d44be82325754ff88423020c81c7032a6fc41cfeb81e982eeab9b",
            "size": "33096140",
            "tarball": "https://ziglang.org/download/0.7.0/zig-linux-aarch64-0.7.0.tar.xz",
        },
        "aarch64-macos": {
            "shasum": "338238035734db74ea4f30e500a4893bf741d38305c10952d5e39fa05bdb057d",
            "size": "33739424",
            "tarball": "https://ziglang.org/download/0.7.0/zig-macos-aarch64-0.7.0.tar.xz",
        },
        "armv7a-linux": {
            "shasum": "011c267e25a96ee160505a560c441daa045359a9d50e13ab1bada9d75c95db2d",
            "size": "35157584",
            "tarball": "https://ziglang.org/download/0.7.0/zig-linux-armv7a-0.7.0.tar.xz",
        },
        "bootstrap": {
            "shasum": "f073beaf5c53c8c57c0d374cbfcb332ef92ad703173edba0d9e0f2ed28401b72",
            "size": "40200436",
            "tarball": "https://ziglang.org/download/0.7.0/zig-bootstrap-0.7.0.tar.xz",
        },
        "date": "2020-11-08",
        "docs": "https://ziglang.org/documentation/0.7.0/",
        "i386-linux": {
            "shasum": "4bb2072cd363bcb1cbeb4872ff5cbc1f683b02d0cc1f90c46e3ea7422ce53222",
            "size": "38530596",
            "tarball": "https://ziglang.org/download/0.7.0/zig-linux-i386-0.7.0.tar.xz",
        },
        "i386-windows": {
            "shasum": "b1e520aacbfbd645ff3521b3eb4d44166d9a0288b8725e4b001f8b50a425eb2e",
            "size": "53390517",
            "tarball": "https://ziglang.org/download/0.7.0/zig-windows-i386-0.7.0.zip",
        },
        "notes": "https://ziglang.org/download/0.7.0/release-notes.html",
        "riscv64-linux": {
            "shasum": "40dff81faa6f232ac40abbf88b9371f3cc932b6e09c423b94387c9ea580cb7be",
            "size": "36759992",
            "tarball": "https://ziglang.org/download/0.7.0/zig-linux-riscv64-0.7.0.tar.xz",
        },
        "src": {
            "shasum": "0efd2cf6c3b05723db80e9cf193bc55150bba84ca41f855a90f53fc756445f83",
            "size": "10683920",
            "tarball": "https://ziglang.org/download/0.7.0/zig-0.7.0.tar.xz",
        },
        "stdDocs": "https://ziglang.org/documentation/0.7.0/std/",
        "x86_64-freebsd": {
            "shasum": "a0c926272ee4ae720034b4a6a1dc98399d76156dd84182554740f0ca8a41fc99",
            "size": "34798992",
            "tarball": "https://ziglang.org/download/0.7.0/zig-freebsd-x86_64-0.7.0.tar.xz",
        },
        "x86_64-linux": {
            "shasum": "e619b1c6094c095b932767f527aee2507f847ea981513ff8a08aab0fd730e0ac",
            "size": "37154432",
            "tarball": "https://ziglang.org/download/0.7.0/zig-linux-x86_64-0.7.0.tar.xz",
        },
        "x86_64-macos": {
            "shasum": "94063f9a311cbbf7a2e0a12295e09437182cf950f18cb0eb30ea9893f3677f24",
            "size": "35258328",
            "tarball": "https://ziglang.org/download/0.7.0/zig-macos-x86_64-0.7.0.tar.xz",
        },
        "x86_64-windows": {
            "shasum": "965f56c0a36f9cda2125e3a348bc654f7f155e2804c3667d231775ec228f8553",
            "size": "53943784",
            "tarball": "https://ziglang.org/download/0.7.0/zig-windows-x86_64-0.7.0.zip",
        },
    },
    "0.7.1": {
        "aarch64-linux": {
            "shasum": "48ec90eba407e4587ddef7eecef25fec7e13587eb98e3b83c5f2f5fff2a5cbe7",
            "size": "33780552",
            "tarball": "https://ziglang.org/download/0.7.1/zig-linux-aarch64-0.7.1.tar.xz",
        },
        "armv7a-linux": {
            "shasum": "5a0662e07b4c4968665e1f97558f8591f6facec45d2e0ff5715e661743107ceb",
            "size": "35813504",
            "tarball": "https://ziglang.org/download/0.7.1/zig-linux-armv7a-0.7.1.tar.xz",
        },
        "bootstrap": {
            "shasum": "040f27c1fae4b0cac0a2782aecdb691f6a2f8e89db6a6ed35024c31c304fd9b2",
            "size": "40232612",
            "tarball": "https://ziglang.org/download/0.7.1/zig-bootstrap-0.7.1.tar.xz",
        },
        "date": "2020-12-13",
        "docs": "https://ziglang.org/documentation/0.7.1/",
        "i386-linux": {
            "shasum": "4882e052e5f83690bd0334bb4fc1702b5403cb3a3d2aa63fd7d6043d8afecba3",
            "size": "39230912",
            "tarball": "https://ziglang.org/download/0.7.1/zig-linux-i386-0.7.1.tar.xz",
        },
        "i386-windows": {
            "shasum": "a1b9a7421e13153e07fd2e2c93ff29aad64d83105b8fcdafa633dbe689caf1c0",
            "size": "54374983",
            "tarball": "https://ziglang.org/download/0.7.1/zig-windows-i386-0.7.1.zip",
        },
        "notes": "https://ziglang.org/download/0.7.1/release-notes.html",
        "riscv64-linux": {
            "shasum": "187294bfd35983348c3fe042901b42e67e7e36ab7f77a5f969d21c0051f4d21f",
            "size": "37454812",
            "tarball": "https://ziglang.org/download/0.7.1/zig-linux-riscv64-0.7.1.tar.xz",
        },
        "src": {
            "shasum": "2db3b944ab368d955b48743d9f7c963b8f96de1a441ba5a35e197237cc6dae44",
            "size": "10711824",
            "tarball": "https://ziglang.org/download/0.7.1/zig-0.7.1.tar.xz",
        },
        "stdDocs": "https://ziglang.org/documentation/0.7.1/std/",
        "x86_64-freebsd": {
            "shasum": "e73c1dca35791a3183fdd5ecde0443ebbe180942efceafe651886034fb8def09",
            "size": "39066808",
            "tarball": "https://ziglang.org/download/0.7.1/zig-freebsd-x86_64-0.7.1.tar.xz",
        },
        "x86_64-linux": {
            "shasum": "18c7b9b200600f8bcde1cd8d7f1f578cbc3676241ce36d771937ce19a8159b8d",
            "size": "37848176",
            "tarball": "https://ziglang.org/download/0.7.1/zig-linux-x86_64-0.7.1.tar.xz",
        },
        "x86_64-macos": {
            "shasum": "845cb17562978af0cf67e3993f4e33330525eaf01ead9386df9105111e3bc519",
            "size": "36211076",
            "tarball": "https://ziglang.org/download/0.7.1/zig-macos-x86_64-0.7.1.tar.xz",
        },
        "x86_64-windows": {
            "shasum": "4818a8a65b4672bc52c0ae7f14d014e0eb8caf10f12c0745176820384cea296a",
            "size": "54909997",
            "tarball": "https://ziglang.org/download/0.7.1/zig-windows-x86_64-0.7.1.zip",
        },
    },
    "0.8.0": {
        "aarch64-linux": {
            "shasum": "ee204ca2c2037952cf3f8b10c609373a08a291efa4af7b3c73be0f2b27720470",
            "size": "37575428",
            "tarball": "https://ziglang.org/download/0.8.0/zig-linux-aarch64-0.8.0.tar.xz",
        },
        "aarch64-macos": {
            "shasum": "b32d13f66d0e1ff740b3326d66a469ee6baddbd7211fa111c066d3bd57683111",
            "size": "35292180",
            "tarball": "https://ziglang.org/download/0.8.0/zig-macos-aarch64-0.8.0.tar.xz",
        },
        "armv7a-linux": {
            "shasum": "d00b8bd97b79f45d6f5da956983bafeaa082e6c2ae8c6e1c6d4faa22fa29b320",
            "size": "38884212",
            "tarball": "https://ziglang.org/download/0.8.0/zig-linux-armv7a-0.8.0.tar.xz",
        },
        "bootstrap": {
            "shasum": "10600bc9c01f92e343f40d6ecc0ad05d67d27c3e382bce75524c0639cd8ca178",
            "size": "43574248",
            "tarball": "https://ziglang.org/download/0.8.0/zig-bootstrap-0.8.0.tar.xz",
        },
        "date": "2021-06-04",
        "docs": "https://ziglang.org/documentation/0.8.0/",
        "i386-linux": {
            "shasum": "96e43ee6ed81c3c63401f456bd1c58ee6d42373a43cb324f5cf4974ca0998865",
            "size": "42136032",
            "tarball": "https://ziglang.org/download/0.8.0/zig-linux-i386-0.8.0.tar.xz",
        },
        "i386-windows": {
            "shasum": "b6ec9aa6cd6f3872fcb30d43ff411802d82008a0c4142ee49e208a09b2c1c5fe",
            "size": "61507213",
            "tarball": "https://ziglang.org/download/0.8.0/zig-windows-i386-0.8.0.zip",
        },
        "notes": "https://ziglang.org/download/0.8.0/release-notes.html",
        "riscv64-linux": {
            "shasum": "75997527a78cdab64c40c43d9df39c01c4cdb557bb3992a869838371a204cfea",
            "size": "40016268",
            "tarball": "https://ziglang.org/download/0.8.0/zig-linux-riscv64-0.8.0.tar.xz",
        },
        "src": {
            "shasum": "03a828d00c06b2e3bb8b7ff706997fd76bf32503b08d759756155b6e8c981e77",
            "size": "12614896",
            "tarball": "https://ziglang.org/download/0.8.0/zig-0.8.0.tar.xz",
        },
        "stdDocs": "https://ziglang.org/documentation/0.8.0/std/",
        "x86_64-freebsd": {
            "shasum": "0d3ccc436c8c0f50fd55462f72f8492d98723c7218ffc2a8a1831967d81b4bdc",
            "size": "39125332",
            "tarball": "https://ziglang.org/download/0.8.0/zig-freebsd-x86_64-0.8.0.tar.xz",
        },
        "x86_64-linux": {
            "shasum": "502625d3da3ae595c5f44a809a87714320b7a40e6dff4a895b5fa7df3391d01e",
            "size": "41211184",
            "tarball": "https://ziglang.org/download/0.8.0/zig-linux-x86_64-0.8.0.tar.xz",
        },
        "x86_64-macos": {
            "shasum": "279f9360b5cb23103f0395dc4d3d0d30626e699b1b4be55e98fd985b62bc6fbe",
            "size": "39969312",
            "tarball": "https://ziglang.org/download/0.8.0/zig-macos-x86_64-0.8.0.tar.xz",
        },
        "x86_64-windows": {
            "shasum": "8580fbbf3afb72e9b495c7f8aeac752a03475ae0bbcf5d787f3775c7e1f4f807",
            "size": "61766193",
            "tarball": "https://ziglang.org/download/0.8.0/zig-windows-x86_64-0.8.0.zip",
        },
    },
    "0.8.1": {
        "aarch64-linux": {
            "shasum": "2166dc9f2d8df387e8b4122883bb979d739281e1ff3f3d5483fec3a23b957510",
            "size": "37605932",
            "tarball": "https://ziglang.org/download/0.8.1/zig-linux-aarch64-0.8.1.tar.xz",
        },
        "aarch64-macos": {
            "shasum": "5351297e3b8408213514b29c0a938002c5cf9f97eee28c2f32920e1227fd8423",
            "size": "35340712",
            "tarball": "https://ziglang.org/download/0.8.1/zig-macos-aarch64-0.8.1.tar.xz",
        },
        "armv7a-linux": {
            "shasum": "5ba58141805e2519f38cf8e715933cbf059f4f3dade92c71838cce341045de05",
            "size": "39185876",
            "tarball": "https://ziglang.org/download/0.8.1/zig-linux-armv7a-0.8.1.tar.xz",
        },
        "bootstrap": {
            "shasum": "fa1239247f830ecd51c42537043f5220e4d1dfefdc54356fa419616a0efb3902",
            "size": "43613464",
            "tarball": "https://ziglang.org/download/0.8.1/zig-bootstrap-0.8.1.tar.xz",
        },
        "date": "2021-09-06",
        "docs": "https://ziglang.org/documentation/0.8.1/",
        "i386-linux": {
            "shasum": "2f3e84f30492b5f1c5f97cecc0166f07a8a8d50c5f85dbb3a6ef2a4ee6f915e6",
            "size": "44782932",
            "tarball": "https://ziglang.org/download/0.8.1/zig-linux-i386-0.8.1.tar.xz",
        },
        "i386-windows": {
            "shasum": "099605051eb0452a947c8eab8fbbc7e43833c8376d267e94e41131c289a1c535",
            "size": "64152358",
            "tarball": "https://ziglang.org/download/0.8.1/zig-windows-i386-0.8.1.zip",
        },
        "notes": "https://ziglang.org/download/0.8.1/release-notes.html",
        "riscv64-linux": {
            "shasum": "4adfaf147b025917c03367462fe5018aaa9edbc6439ef9cd0da2b074ae960554",
            "size": "41234480",
            "tarball": "https://ziglang.org/download/0.8.1/zig-linux-riscv64-0.8.1.tar.xz",
        },
        "src": {
            "shasum": "8c428e14a0a89cb7a15a6768424a37442292858cdb695e2eb503fa3c7bf47f1a",
            "size": "12650228",
            "tarball": "https://ziglang.org/download/0.8.1/zig-0.8.1.tar.xz",
        },
        "stdDocs": "https://ziglang.org/documentation/0.8.1/std/",
        "x86_64-freebsd": {
            "shasum": "fc4f6478bcf3a9fce1b8ef677a91694f476dd35be6d6c9c4f44a8b76eedbe176",
            "size": "39150924",
            "tarball": "https://ziglang.org/download/0.8.1/zig-freebsd-x86_64-0.8.1.tar.xz",
        },
        "x86_64-linux": {
            "shasum": "6c032fc61b5d77a3f3cf781730fa549f8f059ffdb3b3f6ad1c2994d2b2d87983",
            "size": "41250060",
            "tarball": "https://ziglang.org/download/0.8.1/zig-linux-x86_64-0.8.1.tar.xz",
        },
        "x86_64-macos": {
            "shasum": "16b0e1defe4c1807f2e128f72863124bffdd906cefb21043c34b673bf85cd57f",
            "size": "39946200",
            "tarball": "https://ziglang.org/download/0.8.1/zig-macos-x86_64-0.8.1.tar.xz",
        },
        "x86_64-windows": {
            "shasum": "43573db14cd238f7111d6bdf37492d363f11ecd1eba802567a172f277d003926",
            "size": "61897838",
            "tarball": "https://ziglang.org/download/0.8.1/zig-windows-x86_64-0.8.1.zip",
        },
    },
    "0.9.0": {
        "aarch64-linux": {
            "shasum": "1524fedfdbade2dbc9bae1ed98ad38fa7f2114c9a3e94da0d652573c75efbc5a",
            "size": "40008396",
            "tarball": "https://ziglang.org/download/0.9.0/zig-linux-aarch64-0.9.0.tar.xz",
        },
        "aarch64-macos": {
            "shasum": "3991c70594d61d09fb4b316157a7c1d87b1d4ec159e7a5ecd11169ff74cad832",
            "size": "39013392",
            "tarball": "https://ziglang.org/download/0.9.0/zig-macos-aarch64-0.9.0.tar.xz",
        },
        "aarch64-windows": {
            "shasum": "f9018725e3fb2e8992b17c67034726971156eb190685018a9ac8c3a9f7a22340",
            "size": "61461921",
            "tarball": "https://ziglang.org/download/0.9.0/zig-windows-aarch64-0.9.0.zip",
        },
        "armv7a-linux": {
            "shasum": "50225dee6e6448a63ee96383a34d9fe3bba34ae8da1a0c8619bde2cdfc1df87d",
            "size": "41196876",
            "tarball": "https://ziglang.org/download/0.9.0/zig-linux-armv7a-0.9.0.tar.xz",
        },
        "bootstrap": {
            "shasum": "16b0bdf0bc0a5ed1e0950e08481413d806192e06443a512347526647b2baeabc",
            "size": "42557736",
            "tarball": "https://ziglang.org/download/0.9.0/zig-bootstrap-0.9.0.tar.xz",
        },
        "date": "2021-12-20",
        "docs": "https://ziglang.org/documentation/0.9.0/",
        "i386-linux": {
            "shasum": "b0dcf688349268c883292acdd55eaa3c13d73b9146e4b990fad95b84a2ac528b",
            "size": "47408656",
            "tarball": "https://ziglang.org/download/0.9.0/zig-linux-i386-0.9.0.tar.xz",
        },
        "i386-windows": {
            "shasum": "bb839434afc75092015cf4c33319d31463c18512bc01dd719aedf5dcbc368466",
            "size": "67946715",
            "tarball": "https://ziglang.org/download/0.9.0/zig-windows-i386-0.9.0.zip",
        },
        "notes": "https://ziglang.org/download/0.9.0/release-notes.html",
        "riscv64-linux": {
            "shasum": "85466de07504767ed37f59782672ad41bbdf43d6480fafd07f45543278b07620",
            "size": "44171420",
            "tarball": "https://ziglang.org/download/0.9.0/zig-linux-riscv64-0.9.0.tar.xz",
        },
        "src": {
            "shasum": "cd1be83b12f8269cc5965e59877b49fdd8fa638efb6995ac61eb4cea36a2e381",
            "size": "13928772",
            "tarball": "https://ziglang.org/download/0.9.0/zig-0.9.0.tar.xz",
        },
        "stdDocs": "https://ziglang.org/documentation/0.9.0/std/",
        "x86_64-freebsd": {
            "shasum": "c95afe679b7cc4110dc2ecd3606c83a699718b7a958d6627f74c20886333e194",
            "size": "41293236",
            "tarball": "https://ziglang.org/download/0.9.0/zig-freebsd-x86_64-0.9.0.tar.xz",
        },
        "x86_64-linux": {
            "shasum": "5c55344a877d557fb1b28939785474eb7f4f2f327aab55293998f501f7869fa6",
            "size": "43420796",
            "tarball": "https://ziglang.org/download/0.9.0/zig-linux-x86_64-0.9.0.tar.xz",
        },
        "x86_64-macos": {
            "shasum": "c5280eeec4d6e5ea5ce5b448dc9a7c4bdd85ecfed4c1b96aa0835e48b36eccf0",
            "size": "43764596",
            "tarball": "https://ziglang.org/download/0.9.0/zig-macos-x86_64-0.9.0.tar.xz",
        },
        "x86_64-windows": {
            "shasum": "084ea2646850aaf068234b0f1a92b914ed629be47075e835f8a67d55c21d880e",
            "size": "65045849",
            "tarball": "https://ziglang.org/download/0.9.0/zig-windows-x86_64-0.9.0.zip",
        },
    },
    "0.9.1": {
        "aarch64-linux": {
            "shasum": "5d99a39cded1870a3fa95d4de4ce68ac2610cca440336cfd252ffdddc2b90e66",
            "size": "37034860",
            "tarball": "https://ziglang.org/download/0.9.1/zig-linux-aarch64-0.9.1.tar.xz",
        },
        "aarch64-macos": {
            "shasum": "8c473082b4f0f819f1da05de2dbd0c1e891dff7d85d2c12b6ee876887d438287",
            "size": "38995640",
            "tarball": "https://ziglang.org/download/0.9.1/zig-macos-aarch64-0.9.1.tar.xz",
        },
        "aarch64-windows": {
            "shasum": "621bf95f54dc3ff71466c5faae67479419951d7489e40e87fd26d195825fb842",
            "size": "61478151",
            "tarball": "https://ziglang.org/download/0.9.1/zig-windows-aarch64-0.9.1.zip",
        },
        "armv7a-linux": {
            "shasum": "6de64456cb4757a555816611ea697f86fba7681d8da3e1863fa726a417de49be",
            "size": "37974652",
            "tarball": "https://ziglang.org/download/0.9.1/zig-linux-armv7a-0.9.1.tar.xz",
        },
        "bootstrap": {
            "shasum": "0a8e221c71860d8975c15662b3ed3bd863e81c4fe383455a596e5e0e490d6109",
            "size": "42488812",
            "tarball": "https://ziglang.org/download/0.9.1/zig-bootstrap-0.9.1.tar.xz",
        },
        "date": "2022-02-14",
        "docs": "https://ziglang.org/documentation/0.9.1/",
        "i386-linux": {
            "shasum": "e776844fecd2e62fc40d94718891057a1dbca1816ff6013369e9a38c874374ca",
            "size": "44969172",
            "tarball": "https://ziglang.org/download/0.9.1/zig-linux-i386-0.9.1.tar.xz",
        },
        "i386-windows": {
            "shasum": "74a640ed459914b96bcc572183a8db687bed0af08c30d2ea2f8eba03ae930f69",
            "size": "67929868",
            "tarball": "https://ziglang.org/download/0.9.1/zig-windows-i386-0.9.1.zip",
        },
        "notes": "https://ziglang.org/download/0.9.1/release-notes.html",
        "riscv64-linux": {
            "shasum": "208dea53662c2c52777bd9e3076115d2126a4f71aed7f2ff3b8fe224dc3881aa",
            "size": "39390868",
            "tarball": "https://ziglang.org/download/0.9.1/zig-linux-riscv64-0.9.1.tar.xz",
        },
        "src": {
            "shasum": "38cf4e84481f5facc766ba72783e7462e08d6d29a5d47e3b75c8ee3142485210",
            "size": "13940828",
            "tarball": "https://ziglang.org/download/0.9.1/zig-0.9.1.tar.xz",
        },
        "stdDocs": "https://ziglang.org/documentation/0.9.1/std/",
        "x86_64-freebsd": {
            "shasum": "4e06009bd3ede34b72757eec1b5b291b30aa0d5046dadd16ecb6b34a02411254",
            "size": "39028848",
            "tarball": "https://ziglang.org/download/0.9.1/zig-freebsd-x86_64-0.9.1.tar.xz",
        },
        "x86_64-linux": {
            "shasum": "be8da632c1d3273f766b69244d80669fe4f5e27798654681d77c992f17c237d7",
            "size": "41011464",
            "tarball": "https://ziglang.org/download/0.9.1/zig-linux-x86_64-0.9.1.tar.xz",
        },
        "x86_64-macos": {
            "shasum": "2d94984972d67292b55c1eb1c00de46580e9916575d083003546e9a01166754c",
            "size": "43713044",
            "tarball": "https://ziglang.org/download/0.9.1/zig-macos-x86_64-0.9.1.tar.xz",
        },
        "x86_64-windows": {
            "shasum": "443da53387d6ae8ba6bac4b3b90e9fef4ecbe545e1c5fa3a89485c36f5c0e3a2",
            "size": "65047697",
            "tarball": "https://ziglang.org/download/0.9.1/zig-windows-x86_64-0.9.1.zip",
        },
    },
    "master": {
        "aarch64-linux": {
            "shasum": "a90b52a968b9176ab7c2d8fb1b7b84f0e7503dc03d7791d7c5286f1ed9ad5eed",
            "size": "38035988",
            "tarball": "https://ziglang.org/builds/zig-linux-aarch64-0.10.0-dev.4247+3234e8de3.tar.xz",
        },
        "aarch64-macos": {
            "shasum": "d435855e9b62a6aee78e4d707debf137ac3e85a9662ffa47267be56149333f06",
            "size": "40986992",
            "tarball": "https://ziglang.org/builds/zig-macos-aarch64-0.10.0-dev.4247+3234e8de3.tar.xz",
        },
        "date": "2022-10-05",
        "docs": "https://ziglang.org/documentation/master/",
        "src": {
            "shasum": "1fe9fb34ef15d433bd1496782d1a645e3a2122455d6aad0294502ae2f416c7e4",
            "size": "15824808",
            "tarball": "https://ziglang.org/builds/zig-0.10.0-dev.4247+3234e8de3.tar.xz",
        },
        "stdDocs": "https://ziglang.org/documentation/master/std/",
        "version": "0.10.0-dev.4247+3234e8de3",
        "x86_64-freebsd": {
            "shasum": "2555d683f7c8ba903c55c218f64963783f769736b6d6a5a8382e575df82234b5",
            "size": "40954156",
            "tarball": "https://ziglang.org/builds/zig-freebsd-x86_64-0.10.0-dev.4247+3234e8de3.tar.xz",
        },
        "x86_64-linux": {
            "shasum": "5ce6b50eae7a787b7e6e002e3b14cb8a149359df1941cf701e99ded365c9895e",
            "size": "44178932",
            "tarball": "https://ziglang.org/builds/zig-linux-x86_64-0.10.0-dev.4247+3234e8de3.tar.xz",
        },
        "x86_64-macos": {
            "shasum": "81d7a9615b00bce617602fd40fe4e0b3bb962ff0bb4595cf78be067385bce135",
            "size": "44180748",
            "tarball": "https://ziglang.org/builds/zig-macos-x86_64-0.10.0-dev.4247+3234e8de3.tar.xz",
        },
        "x86_64-windows": {
            "shasum": "555ac169c7e35f1dd98ea86bc514e80f7527b242c22d44a34a166c80d4441ceb",
            "size": "69140534",
            "tarball": "https://ziglang.org/builds/zig-windows-x86_64-0.10.0-dev.4247+3234e8de3.zip",
        },
    },
}
