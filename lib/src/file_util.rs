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

use std::fs::File;
use std::path::Path;

use tempfile::{NamedTempFile, PersistError};

// Like NamedTempFile::persist(), but also succeeds if the target already
// exists.
pub fn persist_content_addressed_temp_file<P: AsRef<Path>>(
    temp_file: NamedTempFile,
    new_path: P,
) -> Result<File, PersistError> {
    match temp_file.persist(&new_path) {
        Ok(file) => Ok(file),
        Err(PersistError { error, file }) => {
            if let Ok(existing_file) = File::open(new_path) {
                Ok(existing_file)
            } else {
                Err(PersistError { error, file })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use test_case::test_case;

    use super::*;
    use crate::testutils;

    #[test]
    fn test_persist_no_existing_file() {
        let temp_dir = testutils::new_temp_dir();
        let target = temp_dir.path().join("file");
        let mut temp_file = NamedTempFile::new_in(&temp_dir).unwrap();
        temp_file.write_all(b"contents").unwrap();
        assert!(persist_content_addressed_temp_file(temp_file, &target).is_ok());
    }

    #[test_case(false ; "existing file open")]
    #[test_case(true ; "existing file closed")]
    fn test_persist_target_exists(existing_file_closed: bool) {
        let temp_dir = testutils::new_temp_dir();
        let target = temp_dir.path().join("file");
        let mut temp_file = NamedTempFile::new_in(&temp_dir).unwrap();
        temp_file.write_all(b"contents").unwrap();

        let mut file = File::create(&target).unwrap();
        file.write_all(b"contents").unwrap();
        if existing_file_closed {
            drop(file);
        }

        assert!(persist_content_addressed_temp_file(temp_file, &target).is_ok());
    }
}
