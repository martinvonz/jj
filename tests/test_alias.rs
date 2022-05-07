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

use crate::common::TestEnvironment;

pub mod common;

#[test]
fn test_alias_invalid_definition() {
    let test_env = TestEnvironment::default();

    test_env.add_config(
        br#"[alias]
    non-list = 5
    non-string-list = [7]
    "#,
    );
    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["non-list"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Alias definition for "non-list" must be a string list
    "###);
    let stderr = test_env.jj_cmd_failure(test_env.env_root(), &["non-string-list"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Alias definition for "non-string-list" must be a string list
    "###);
}
