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

use crate::common::TestEnvironment;

#[test]
fn test_sign() {
    let test_env = TestEnvironment::default();

    test_env.add_config(
        r#"
[signing]
show-signatures = true
sign-all = false
backend = "mock"
"#,
    );

    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "init"]);

    let show_no_sig = test_env.jj_cmd_success(&repo_path, &["show", "-r", "@-"]);

    insta::assert_snapshot!(show_no_sig, @r###"
    Commit ID: 9f2e994e4ee015d1b91f6676bc2de9531efb98fd
    Change ID: qpvuntsmwlqtpsluzzsnyyzlmlwvmlnu
    Author: Test User <test.user@example.com> (2001-02-03 04:05:07.000 +07:00)
    Committer: Test User <test.user@example.com> (2001-02-03 04:05:08.000 +07:00)

        init
    "###);

    let (_, stderr) = test_env.jj_cmd_ok(&repo_path, &["sign", "-r", "@-"]);
    insta::assert_snapshot!(stderr, @r###"
    Rebased 1 descendant commits
    Working copy now at: rlvkpnrz b162855d (empty) (no description set)
    Parent commit      : qpvuntsm [✓︎] 5aab9df2 (empty) init
    Commit was signed: qpvuntsm [✓︎] 5aab9df2 (empty) init
    "###);

    let show_with_sig = test_env.jj_cmd_success(&repo_path, &["show", "-r", "@-"]);

    insta::assert_snapshot!(show_with_sig, @r###"
    Commit ID: 5aab9df27eb838f225ae554edd56a11b3ecd13df
    Change ID: qpvuntsmwlqtpsluzzsnyyzlmlwvmlnu
    Author: Test User <test.user@example.com> (2001-02-03 04:05:07.000 +07:00)
    Committer: Test User <test.user@example.com> (2001-02-03 04:05:10.000 +07:00)
    Signature: Good mock signature

        init
    "###);
}

#[test]
fn test_sig_drop() {
    let test_env = TestEnvironment::default();

    test_env.add_config(
        r#"
[signing]
show-signatures = true
sign-all = false
backend = "mock"
"#,
    );

    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");
    test_env.jj_cmd_ok(&repo_path, &["commit", "-m", "init"]);

    let show_no_sig = test_env.jj_cmd_success(&repo_path, &["show", "-r", "@-"]);
    insta::assert_snapshot!(show_no_sig, @r###"
    Commit ID: 9f2e994e4ee015d1b91f6676bc2de9531efb98fd
    Change ID: qpvuntsmwlqtpsluzzsnyyzlmlwvmlnu
    Author: Test User <test.user@example.com> (2001-02-03 04:05:07.000 +07:00)
    Committer: Test User <test.user@example.com> (2001-02-03 04:05:08.000 +07:00)

        init
    "###);

    test_env.jj_cmd_ok(&repo_path, &["sign", "-r", "@-"]);

    let show_with_sig = test_env.jj_cmd_success(&repo_path, &["show", "-r", "@-"]);
    insta::assert_snapshot!(show_with_sig, @r###"
    Commit ID: 5aab9df27eb838f225ae554edd56a11b3ecd13df
    Change ID: qpvuntsmwlqtpsluzzsnyyzlmlwvmlnu
    Author: Test User <test.user@example.com> (2001-02-03 04:05:07.000 +07:00)
    Committer: Test User <test.user@example.com> (2001-02-03 04:05:10.000 +07:00)
    Signature: Good mock signature

        init
    "###);

    test_env.jj_cmd_ok(&repo_path, &["sign", "-r", "@-", "--drop"]);

    let show_with_sig = test_env.jj_cmd_success(&repo_path, &["show", "-r", "@-"]);
    insta::assert_snapshot!(show_with_sig, @r###"
    Commit ID: a37490e69293173538209a45786d10c63c8960d7
    Change ID: qpvuntsmwlqtpsluzzsnyyzlmlwvmlnu
    Author: Test User <test.user@example.com> (2001-02-03 04:05:07.000 +07:00)
    Committer: Test User <test.user@example.com> (2001-02-03 04:05:12.000 +07:00)

        init
    "###);
}
