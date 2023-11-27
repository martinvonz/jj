// Copyright 2022 The Jujutsu Authors
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

#[test]
fn test_signature_templates() {
    let test_env = TestEnvironment::default();

    test_env.add_config(r#"signing.sign-all = true"#);
    test_env.add_config(r#"signing.backend = "mock""#);

    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    let stdout = test_env.jj_cmd_success(&repo_path, &["log", "-Tbuiltin_log_detailed_with_sig"]);
    insta::assert_snapshot!(stdout, @r###"
    @  Commit ID: ab2be60e0fe849f40d8bcce25f93ec4216e9c488
    │  Change ID: qpvuntsmwlqtpsluzzsnyyzlmlwvmlnu
    │  Author: Test User <test.user@example.com> (2001-02-03 04:05:07.000 +07:00)
    │  Committer: Test User <test.user@example.com> (2001-02-03 04:05:07.000 +07:00)
    │  Signature: Good mock signature
    │
    │      (no description set)
    │
    ◉  Commit ID: 0000000000000000000000000000000000000000
       Change ID: zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz
       Author: (no name set) <(no email set)> (1970-01-01 00:00:00.000 +00:00)
       Committer: (no name set) <(no email set)> (1970-01-01 00:00:00.000 +00:00)
    
           (no description set)
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["show", "-Tbuiltin_log_detailed_with_sig"]);
    insta::assert_snapshot!(stdout, @r###"
    Commit ID: ab2be60e0fe849f40d8bcce25f93ec4216e9c488
    Change ID: qpvuntsmwlqtpsluzzsnyyzlmlwvmlnu
    Author: Test User <test.user@example.com> (2001-02-03 04:05:07.000 +07:00)
    Committer: Test User <test.user@example.com> (2001-02-03 04:05:07.000 +07:00)
    Signature: Good mock signature
    
        (no description set)
    "###);
}
