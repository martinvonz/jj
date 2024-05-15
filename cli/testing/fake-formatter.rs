// Copyright 2024 The Jujutsu Authors
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

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{arg, Parser};
use itertools::Itertools;

/// A fake code formatter, useful for testing
///
/// `fake-formatter` is similar to `cat`.
/// `fake-formatter --reverse` is similar to `rev` (not `tac`).
/// `fake-formatter --stdout foo` is similar to `echo foo`.
/// `fake-formatter --stdout foo --stderr bar --fail` is similar to
///   `echo foo; echo bar >&2; false`.
/// `fake-formatter --tee foo` is similar to `tee foo`).
///
/// This program acts as a portable alternative to that class of shell commands.
#[derive(Parser, Debug)]
struct Args {
    /// Exit with non-successful status.
    #[arg(long, default_value_t = false)]
    fail: bool,

    /// Reverse the characters in each line when reading stdin.
    #[arg(long, default_value_t = false)]
    reverse: bool,

    /// Convert all characters to uppercase when reading stdin.
    #[arg(long, default_value_t = false)]
    uppercase: bool,

    /// Write this string to stdout, and ignore stdin.
    #[arg(long)]
    stdout: Option<String>,

    /// Write this string to stderr.
    #[arg(long)]
    stderr: Option<String>,

    /// Duplicate stdout into this file.
    #[arg(long)]
    tee: Option<PathBuf>,
}

fn main() -> ExitCode {
    let args: Args = Args::parse();
    // Code formatters tend to print errors before printing the result.
    if let Some(data) = args.stderr {
        eprint!("{}", data);
    }
    let stdout = if let Some(data) = args.stdout {
        data // --reverse doesn't apply to --stdout.
    } else {
        std::io::stdin()
            .lines()
            .map(|line| {
                format!("{}\n", {
                    let line = if args.reverse {
                        line.unwrap().chars().rev().collect()
                    } else {
                        line.unwrap()
                    };
                    if args.uppercase {
                        line.to_uppercase()
                    } else {
                        line
                    }
                })
            })
            .join("")
    };
    print!("{}", stdout);
    if let Some(path) = args.tee {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        write!(file, "{}", stdout).unwrap();
    }
    if args.fail {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
