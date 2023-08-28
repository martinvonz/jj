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

use std::fmt::Debug;
use std::path::PathBuf;

pub trait SubmoduleStore: Send + Sync + Debug {
    fn name(&self) -> &str;
    // FIXME This is a quick hack to experiment with git clone. Now we just pass
    // the path to the high level git clone machinery, but in the long run,
    // we're more likely to move git clone machinery into the SubmoduleStore
    // implementation and replace this function with something like
    // clone_submodule().
    //
    // Given the name of a submodule, return the path that it should be cloned
    // to (for consumption by the `jj git clone` machinery).
    fn get_submodule_path(&self, submodule: &str) -> PathBuf;
}
