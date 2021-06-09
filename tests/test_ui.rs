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

use std::io::Cursor;

use jujutsu::ui::{FilePathParseError, Ui};
use jujutsu_lib::repo_path::RepoPath;
use jujutsu_lib::testutils::user_settings;

#[test]
fn test_parse_file_path_wc_in_cwd() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cwd_path = temp_dir.path().join("repo");
    let wc_path = cwd_path.clone();
    let mut unused_stdout_buf = vec![];
    let unused_stdout = Box::new(Cursor::new(&mut unused_stdout_buf));
    let ui = Ui::new(cwd_path, unused_stdout, false, user_settings());

    assert_eq!(ui.parse_file_path(&wc_path, ""), Ok(RepoPath::root()));
    assert_eq!(ui.parse_file_path(&wc_path, "."), Ok(RepoPath::root()));
    assert_eq!(
        ui.parse_file_path(&wc_path, "file"),
        Ok(RepoPath::from_internal_string("file"))
    );
    // Both slash and the platform's separator are allowed
    assert_eq!(
        ui.parse_file_path(&wc_path, &format!("dir{}file", std::path::MAIN_SEPARATOR)),
        Ok(RepoPath::from_internal_string("dir/file"))
    );
    assert_eq!(
        ui.parse_file_path(&wc_path, "dir/file"),
        Ok(RepoPath::from_internal_string("dir/file"))
    );
    assert_eq!(
        ui.parse_file_path(&wc_path, ".."),
        Err(FilePathParseError::InputNotInRepo("..".to_string()))
    );
    // TODO: handle these cases:
    // assert_eq!(ui.parse_file_path(&cwd_path, "../repo"),
    // Ok(RepoPath::root())); assert_eq!(ui.parse_file_path(&cwd_path,
    // "../repo/file"), Ok(RepoPath::from_internal_string("file")));
}

#[test]
fn test_parse_file_path_wc_in_cwd_parent() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cwd_path = temp_dir.path().join("dir");
    let wc_path = cwd_path.parent().unwrap().to_path_buf();
    let mut unused_stdout_buf = vec![];
    let unused_stdout = Box::new(Cursor::new(&mut unused_stdout_buf));
    let ui = Ui::new(cwd_path, unused_stdout, false, user_settings());

    assert_eq!(
        ui.parse_file_path(&wc_path, ""),
        Ok(RepoPath::from_internal_string("dir"))
    );
    assert_eq!(
        ui.parse_file_path(&wc_path, "."),
        Ok(RepoPath::from_internal_string("dir"))
    );
    assert_eq!(
        ui.parse_file_path(&wc_path, "file"),
        Ok(RepoPath::from_internal_string("dir/file"))
    );
    assert_eq!(
        ui.parse_file_path(&wc_path, "subdir/file"),
        Ok(RepoPath::from_internal_string("dir/subdir/file"))
    );
    assert_eq!(ui.parse_file_path(&wc_path, ".."), Ok(RepoPath::root()));
    assert_eq!(
        ui.parse_file_path(&wc_path, "../.."),
        Err(FilePathParseError::InputNotInRepo("../..".to_string()))
    );
    assert_eq!(
        ui.parse_file_path(&wc_path, "../other-dir/file"),
        Ok(RepoPath::from_internal_string("other-dir/file"))
    );
}

#[test]
fn test_parse_file_path_wc_in_cwd_child() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cwd_path = temp_dir.path().join("cwd");
    let wc_path = cwd_path.join("repo");
    let mut unused_stdout_buf = vec![];
    let unused_stdout = Box::new(Cursor::new(&mut unused_stdout_buf));
    let ui = Ui::new(cwd_path, unused_stdout, false, user_settings());

    assert_eq!(
        ui.parse_file_path(&wc_path, ""),
        Err(FilePathParseError::InputNotInRepo("".to_string()))
    );
    assert_eq!(
        ui.parse_file_path(&wc_path, "not-repo"),
        Err(FilePathParseError::InputNotInRepo("not-repo".to_string()))
    );
    assert_eq!(ui.parse_file_path(&wc_path, "repo"), Ok(RepoPath::root()));
    assert_eq!(
        ui.parse_file_path(&wc_path, "repo/file"),
        Ok(RepoPath::from_internal_string("file"))
    );
    assert_eq!(
        ui.parse_file_path(&wc_path, "repo/dir/file"),
        Ok(RepoPath::from_internal_string("dir/file"))
    );
}
