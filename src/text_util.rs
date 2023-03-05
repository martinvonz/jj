// Copyright 2022-2023 The Jujutsu Authors
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

use std::io;

use crate::formatter::{FormatRecorder, Formatter};

pub fn complete_newline(s: impl Into<String>) -> String {
    let mut s = s.into();
    if !s.is_empty() && !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// Indents each line by the given prefix preserving labels.
pub fn write_indented(
    formatter: &mut dyn Formatter,
    recorded_content: &FormatRecorder,
    mut write_prefix: impl FnMut(&mut dyn Formatter) -> io::Result<()>,
) -> io::Result<()> {
    let mut new_line = true;
    recorded_content.replay_with(formatter, |formatter, data| {
        for line in data.split_inclusive(|&c| c == b'\n') {
            if new_line && line != b"\n" {
                // Prefix inherits the current labels. This is implementation detail
                // and may be fixed later.
                write_prefix(formatter)?;
            }
            formatter.write_all(line)?;
            new_line = line.ends_with(b"\n");
        }
        Ok(())
    })
}
