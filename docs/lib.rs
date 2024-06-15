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

use std::borrow::Cow;

#[derive(rust_embed::RustEmbed)]
#[folder = "."]
#[include = "*.md"]
struct DocAssetsMd;

pub struct DocAssets;

/// Documentation assets, allowing you to look up and iterate all the documents
/// available.
impl DocAssets {
    // This is a simple wrapper around the `DocAssetsMd`
    // struct that handles trimming off Markdown `.md` extensions, so that users
    // can refer to documentation items without needing to know the file extension.

    /// Iterator. Returns all the documentation items available.
    pub fn iter() -> impl Iterator<Item = String> {
        DocAssetsMd::iter().map(|name| name.trim_end_matches(".md").to_owned())
    }

    pub fn get(name: &str) -> Option<Cow<'static, [u8]>> {
        // re-attach the `.md` extension before lookup
        DocAssetsMd::get(&format!("{}.md", name)).map(|data| data.data)
    }
}
