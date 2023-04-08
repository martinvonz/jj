// Copyright 2023 The Jujutsu Authors
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

use std::process::Command;
use std::str;

use cargo_metadata::MetadataCommand;

fn main() -> std::io::Result<()> {
    let path = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let meta = MetadataCommand::new()
        .manifest_path("./Cargo.toml")
        .current_dir(&path)
        .exec()
        .unwrap();
    let root = meta.root_package().unwrap();
    let version = &root.version;

    if let Some(git_hash) = get_git_hash() {
        println!("cargo:rustc-env=JJ_VERSION={}-{}", version, git_hash);
    } else {
        println!("cargo:rustc-env=JJ_VERSION={}", version);
    }

    Ok(())
}

fn get_git_hash() -> Option<String> {
    if let Ok(output) = Command::new("jj")
        .args([
            "--ignore-working-copy",
            "log",
            "--no-graph",
            "-r=@-",
            "-T=commit_id",
        ])
        .output()
    {
        if output.status.success() {
            println!("cargo:rerun-if-changed=.jj/repo/op_heads/heads/");
            return Some(String::from_utf8(output.stdout).unwrap());
        }
    }

    if let Ok(output) = Command::new("git").args(["rev-parse", "HEAD"]).output() {
        if output.status.success() {
            println!("cargo:rerun-if-changed=.git/HEAD");
            let line = str::from_utf8(&output.stdout).unwrap();
            return Some(line.trim_end().to_owned());
        }
    }

    None
}
