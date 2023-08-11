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

extern crate chrono;

use std::path::Path;
use std::process::Command;
use std::str;

use cargo_metadata::MetadataCommand;
use chrono::prelude::*;

const GIT_HEAD_PATH: &str = "../.git/HEAD";
const JJ_OP_HEADS_PATH: &str = "../.jj/repo/op_heads/heads";

fn main() -> std::io::Result<()> {
    let path = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let meta = MetadataCommand::new()
        .manifest_path("./Cargo.toml")
        .current_dir(&path)
        .exec()
        .unwrap();
    let root = meta.root_package().unwrap();
    let version = &root.version;

    if Path::new(GIT_HEAD_PATH).exists() {
        // In colocated repo, .git/HEAD should reflect the working-copy parent.
        println!("cargo:rerun-if-changed={GIT_HEAD_PATH}");
    } else if Path::new(JJ_OP_HEADS_PATH).exists() {
        // op_heads changes when working-copy files are mutated, which is way more
        // frequent than .git/HEAD.
        println!("cargo:rerun-if-changed={JJ_OP_HEADS_PATH}");
    }
    println!("cargo:rerun-if-env-changed=NIX_JJ_GIT_HASH");

    // TODO: timestamp can be "nix"
    if let Some((git_hash, maybe_date)) = get_git_timestamp_and_hash() {
        println!(
            "cargo:rustc-env=JJ_VERSION={}-{}-{}",
            version,
            maybe_date
                .map(|d| d.format("%Y%m%d").to_string())
                .unwrap_or_else(|| "dateunknown".to_string()),
            git_hash
        );
    } else {
        println!("cargo:rustc-env=JJ_VERSION={}", version);
    }

    Ok(())
}

/// Convert a string with a unix timestamp to a date
fn timestamp_to_date(ts_str: &str) -> Option<DateTime<Utc>> {
    ts_str
        .parse::<i64>()
        .ok()
        .and_then(|ts| DateTime::<Utc>::from_timestamp(ts, 0))
}

/// Return the git hash and the committer timestamp
fn get_git_timestamp_and_hash() -> Option<(String, Option<DateTime<Utc>>)> {
    if let Some(nix_hash) = std::env::var("NIX_JJ_GIT_HASH")
        .ok()
        .filter(|s| !s.is_empty())
    {
        return Some((nix_hash, None));
    }

    fn parse_timestamp_vbar_hash(bytes: &[u8]) -> (String, Option<DateTime<Utc>>) {
        let s = str::from_utf8(bytes).unwrap().trim_end();
        let (ts_str, id) = s.split_once('|').unwrap();
        (id.to_owned(), timestamp_to_date(ts_str))
    }
    if let Ok(output) = Command::new("jj")
        .args([
            "--ignore-working-copy",
            "--color=never",
            "log",
            "--no-graph",
            "-r=@-",
            r#"-T=committer.timestamp().utc().format("%s") ++ "|" ++ commit_id"#,
        ])
        .output()
    {
        if output.status.success() {
            return Some(parse_timestamp_vbar_hash(&output.stdout));
        }
    }

    if let Ok(output) = Command::new("git")
        .args(["log", "-1", "--format=%ct|%H", "HEAD"])
        .output()
    {
        if output.status.success() {
            return Some(parse_timestamp_vbar_hash(&output.stdout));
        }
    }

    None
}
