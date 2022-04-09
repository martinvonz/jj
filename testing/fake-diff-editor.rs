// Copyright 2022 Google LLC
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

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::exit;

use clap::Parser;
use itertools::Itertools;

/// A fake diff-editor, useful for testing
#[derive(Parser, Debug)]
#[clap()]
struct Args {
    /// Path to the "before" directory
    before: PathBuf,

    /// Path to the "after" directory
    after: PathBuf,
}

fn files_recursively(dir: &PathBuf) -> HashSet<String> {
    let mut files = HashSet::new();
    for dir_entry in std::fs::read_dir(dir).unwrap() {
        let dir_entry = dir_entry.unwrap();
        let base_name = dir_entry.file_name().to_str().unwrap().to_string();
        if dir_entry.path().is_dir() {
            for sub_path in files_recursively(&dir_entry.path()) {
                files.insert(format!("{base_name}/{sub_path}"));
            }
        } else {
            files.insert(base_name);
        }
    }
    files
}

fn main() {
    let args: Args = Args::parse();
    let edit_script_path = PathBuf::from(std::env::var_os("DIFF_EDIT_SCRIPT").unwrap());
    let edit_script = String::from_utf8(std::fs::read(&edit_script_path).unwrap()).unwrap();
    for instruction in edit_script.split('\0') {
        let (command, payload) = instruction.split_once('\n').unwrap_or((instruction, ""));
        let parts = command.split(' ').collect_vec();
        match parts.as_slice() {
            [""] => {}
            ["fail"] => exit(1),
            ["files-before", ..] => {
                let expected = parts[1..].iter().copied().map(str::to_string).collect();
                let actual = files_recursively(&args.before);
                if actual != expected {
                    eprintln!(
                        "unexpected files before: {:?}",
                        actual.iter().sorted().collect_vec()
                    );
                    exit(1)
                }
            }
            ["files-after", ..] => {
                let expected = parts[1..].iter().copied().map(str::to_string).collect();
                let actual = files_recursively(&args.after);
                if actual != expected {
                    eprintln!(
                        "unexpected files after: {:?}",
                        actual.iter().sorted().collect_vec()
                    );
                    exit(1)
                }
            }
            ["rm", file] => {
                std::fs::remove_file(args.after.join(file)).unwrap();
            }
            ["reset", file] => {
                if args.before.join(file).exists() {
                    std::fs::copy(&args.before.join(file), &args.after.join(file)).unwrap();
                } else {
                    std::fs::remove_file(args.after.join(file)).unwrap();
                }
            }
            ["write", file] => {
                std::fs::write(args.after.join(file), payload).unwrap();
            }
            _ => {
                eprintln!("unexpected command: {}", command);
                exit(1)
            }
        }
    }
}
