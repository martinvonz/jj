// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

use std::io::Result;
use std::path::Path;

fn main() -> Result<()> {
    let input = [
        "git_store.proto",
        "local_store.proto",
        "op_store.proto",
        "working_copy.proto",
    ];

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let protos_dir = root.join("src").join("protos");

    prost_build::Config::new()
        .out_dir(&protos_dir)
        .include_file("mod.rs")
        // For old protoc versions. 3.12.4 needs this, but 3.21.12 doesn't.
        .protoc_arg("--experimental_allow_proto3_optional")
        .compile_protos(
            &input
                .into_iter()
                .map(|x| protos_dir.join(x))
                .collect::<Vec<_>>(),
            &[protos_dir],
        )
}
