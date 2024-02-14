// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

use std::path::PathBuf;
use std::process::exit;
use std::{env, fs};

use clap::Parser;
use itertools::Itertools;

/// A fake editor, useful for testing
// It's overkill to use clap for a single argument, but we already use it in many other places...
#[derive(Parser, Debug)]
#[clap()]
struct Args {
    /// Path to the file to edit
    file: PathBuf,
}

fn main() {
    let args: Args = Args::parse();
    let edit_script_path = PathBuf::from(env::var_os("EDIT_SCRIPT").unwrap());
    let edit_script = fs::read_to_string(&edit_script_path).unwrap();

    let mut instructions = edit_script.split('\0').collect_vec();
    if let Some(pos) = instructions.iter().position(|&i| i == "next invocation\n") {
        // Overwrite the edit script. The next time `fake-editor` is called, it will
        // only see the part after the `next invocation` command.
        fs::write(&edit_script_path, instructions[pos + 1..].join("\0")).unwrap();
        instructions.truncate(pos);
    }
    for instruction in instructions {
        let (command, payload) = instruction.split_once('\n').unwrap_or((instruction, ""));
        let parts = command.split(' ').collect_vec();
        match parts.as_slice() {
            [""] => {}
            ["fail"] => exit(1),
            ["dump", dest] => {
                let dest_path = edit_script_path.parent().unwrap().join(dest);
                fs::copy(&args.file, dest_path).unwrap();
            }
            ["expect"] => {
                let actual = String::from_utf8(fs::read(&args.file).unwrap()).unwrap();
                if actual != payload {
                    eprintln!("fake-editor: Unexpected content.\n");
                    eprintln!("EXPECTED: <{payload}>\nRECEIVED: <{actual}>");
                    exit(1)
                }
            }
            ["expectpath"] => {
                let actual = args.file.to_str().unwrap();
                if actual != payload {
                    eprintln!("fake-editor: Unexpected path.\n");
                    eprintln!("EXPECTED: <{payload}>\nRECEIVED: <{actual}>");
                    exit(1)
                }
            }
            ["write"] => {
                fs::write(&args.file, payload).unwrap_or_else(|_| {
                    panic!("Failed to write file {}", args.file.to_str().unwrap())
                });
            }
            _ => {
                eprintln!("fake-editor: unexpected command: {command}");
                exit(1)
            }
        }
    }
}
