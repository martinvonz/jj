// Copyright 2024 The Jujutsu Authors
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

//! Support for `.mailmap` files.

use std::fmt::Debug;
use std::io::{self, Read};

use pollster::FutureExt;

use crate::backend::Signature;
use crate::commit::Commit;
use crate::conflicts::{materialize_tree_value, MaterializedTreeValue};
use crate::op_store::WorkspaceId;
use crate::repo::Repo;
use crate::repo_path::RepoPath;

/// Models a `.mailmap` file, mapping email addresses and names to
/// canonical ones.
///
/// The syntax and semantics are as described in
/// [`gitmailmap(5)`](https://git-scm.com/docs/gitmailmap).
///
/// You can obtain the currently‐applicable [`Mailmap`] using
/// [`get_current_mailmap`].
///
/// An empty [`Mailmap`] does not use any heap allocations, and an absent
/// `.mailmap` file is semantically equivalent to an empty one, so there is
/// usually no need to wrap this type in [`Option`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mailmap(gix_mailmap::Snapshot);

impl Mailmap {
    /// Parses a `.mailmap` file, ignoring parse errors.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(gix_mailmap::Snapshot::from_bytes(bytes))
    }

    /// Reads and parses a `.mailmap` file from a reader, ignoring parse errors.
    pub fn from_reader<R: Read>(reader: &mut R) -> io::Result<Self> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        Ok(Self::from_bytes(&bytes))
    }

    /// Returns the canonical signature corresponding to the raw `signature`.
    /// The timestamp is left untouched. Signatures with no corresponding entry
    /// are returned as‐is.
    pub fn resolve(&self, signature: &Signature) -> Signature {
        let result = self.0.try_resolve(gix_actor::SignatureRef {
            name: signature.name.as_bytes().into(),
            email: signature.email.as_bytes().into(),
            time: Default::default(),
        });
        match result {
            Some(canonical) => Signature {
                name: String::from_utf8_lossy(&canonical.name).into(),
                email: String::from_utf8_lossy(&canonical.email).into(),
                timestamp: signature.timestamp.clone(),
            },
            None => signature.clone(),
        }
    }

    /// Returns the canonical author signature of `commit`.
    pub fn author(&self, commit: &Commit) -> Signature {
        self.resolve(commit.author_raw())
    }

    /// Returns the canonical committer signature of `commit`.
    pub fn committer(&self, commit: &Commit) -> Signature {
        self.resolve(commit.committer_raw())
    }
}

/// Reads and parses the `.mailmap` file from the working‐copy commit of the
/// specified workspace. An absent `.mailmap` is treated the same way
/// as an empty file, and any errors finding or materializing the file are
/// treated the same way. Parse errors when reading the file are ignored, but
/// the rest of the file will still be processed.
pub fn get_current_mailmap(repo: &dyn Repo, workspace_id: &WorkspaceId) -> Mailmap {
    // TODO: Figure out if any errors here should be handled or surfaced.
    let inner = || {
        let commit_id = repo.view().get_wc_commit_id(workspace_id)?;
        let commit = repo.store().get_commit(commit_id).ok()?;
        let tree = commit.tree().ok()?;
        let path = RepoPath::from_internal_string(".mailmap");
        let value = tree.path_value(path).ok()?;
        // We ignore symbolic links, as per `gitmailmap(5)`.
        //
        // TODO: Figure out how conflicts should be handled here.
        let materialized = materialize_tree_value(repo.store(), path, value)
            .block_on()
            .ok()?;
        let MaterializedTreeValue::File { mut reader, .. } = materialized else {
            return None;
        };
        Mailmap::from_reader(&mut reader).ok()
    };
    inner().unwrap_or_default()
}
