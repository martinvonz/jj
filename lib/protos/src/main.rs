// Copyright 2020 Google LLC
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

extern crate protobuf_codegen_pure;

use protobuf_codegen_pure::Codegen;

fn main() {
    Codegen::new()
        .out_dir("src/")
        .include("src/")
        .input("src/op_store.proto")
        .input("src/store.proto")
        .input("src/working_copy.proto")
        .run()
        .expect("protoc");
}
