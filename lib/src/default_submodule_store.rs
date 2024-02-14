// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

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
        Self::name()
    }
}
