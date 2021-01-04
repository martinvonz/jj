// Copyright 2021 Google LLC
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

use jujube_lib::repo::{ReadonlyRepo, RepoLoadError};
use jujube_lib::testutils;

#[test]
fn test_load_bad_path() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let wc_path = temp_dir.path().to_owned();
    // We haven't created a repo in the wc_path, so it should fail to load.
    let result = ReadonlyRepo::load(&settings, wc_path.clone());
    assert_eq!(result.err(), Some(RepoLoadError::NoRepoHere(wc_path)));
}
