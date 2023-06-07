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

#![allow(missing_docs)]

use std::path::{Path, PathBuf};

use crate::submodule_store::SubmoduleStore;

#[derive(Debug)]
pub struct DefaultSubmoduleStore {
    #[allow(dead_code)]
    path: PathBuf,
}

impl DefaultSubmoduleStore {
    /// Load an existing SubmoduleStore
    pub fn load(store_path: &Path) -> Self {
        DefaultSubmoduleStore {
            path: store_path.to_path_buf(),
        }
    }

    pub fn init(store_path: &Path) -> Self {
        DefaultSubmoduleStore {
            path: store_path.to_path_buf(),
        }
    }

    pub fn name() -> &'static str {
        "default"
    }
}

impl SubmoduleStore for DefaultSubmoduleStore {
    fn name(&self) -> &str {
        DefaultSubmoduleStore::name()
    }

    fn get_submodule_path(&self, submodule: &str) -> PathBuf {
        PathBuf::new()
            .join(self.path.clone())
            // FIXME hackily sanitze "/" in the path. It's likely that we'll
            // need to do reverse lookups (e.g. from 'sanitized' to the 'real'
            // name), so this probably won't do in the long run.
            .join(submodule.to_string().replace("/", "__"))
    }
}
