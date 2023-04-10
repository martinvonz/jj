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

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::fs;
use std::path::PathBuf;

use prost::Message;
use thiserror::Error;

use crate::backend::{CommitId, ObjectId};

// TODO: consider making this more expressive. Currently, it's based on
// OpStoreError
#[derive(Debug, Error)]
pub enum GitRefViewError {
    #[error("{0}")]
    Other(String),
}

impl From<std::io::Error> for GitRefViewError {
    fn from(err: std::io::Error) -> Self {
        GitRefViewError::Other(err.to_string())
    }
}

impl From<prost::DecodeError> for GitRefViewError {
    fn from(err: prost::DecodeError) -> Self {
        GitRefViewError::Other(err.to_string())
    }
}

pub type RefName = String;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitRefView {
    pub refs: BTreeMap<RefName, CommitId>,
}

type GitRefViewProto = crate::protos::git_ref_view::GitRefView;
type GitRefProto = crate::protos::git_ref_view::GitRef;

impl GitRefView {
    pub fn read_view_from_file(view_path: PathBuf) -> Result<Self, GitRefViewError> {
        let buf = fs::read(view_path)?;

        let proto = GitRefViewProto::decode(&*buf)?;
        Ok(view_from_proto(proto))
    }

    pub fn write_view_to_file(&self, path: PathBuf) -> Result<(), GitRefViewError> {
        let proto = view_to_proto(self);
        Ok(std::fs::write(path, proto.encode_to_vec())?)
    }
}

fn view_to_proto(ref_view: &GitRefView) -> GitRefViewProto {
    let mut proto = GitRefViewProto::default();
    for (ref_name, commit_id) in &ref_view.refs {
        proto.refs.push(GitRefProto {
            name: ref_name.clone(),
            commit_id: commit_id.to_bytes(),
        });
    }

    proto
}

fn view_from_proto(proto: GitRefViewProto) -> GitRefView {
    let mut view = GitRefView::default();
    for GitRefProto {
        name,
        commit_id: id_bytes,
    } in proto.refs
    {
        view.refs.insert(name, CommitId::new(id_bytes));
    }

    view
}

#[cfg(test)]
mod tests {

    use maplit::btreemap;

    use super::*;
    use crate::backend::{CommitId, ObjectId};

    fn create_view() -> GitRefView {
        GitRefView {
            refs: btreemap! {
                "ref1".to_string() => CommitId::from_hex("aaa111"),
                "ref2".to_string() => CommitId::from_hex("aaa222"),
            },
        }
    }

    #[test]
    fn test_read_write_view() {
        let temp_file = testutils::new_temp_dir().into_path().join("git_ref_view");
        let view = create_view();
        view.write_view_to_file(temp_file.clone()).unwrap();
        let read_view = GitRefView::read_view_from_file(temp_file).unwrap();
        assert_eq!(read_view, view);
    }
}
