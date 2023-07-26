// Copyright 2022 The Jujutsu Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

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
