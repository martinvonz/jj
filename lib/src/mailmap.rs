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

use std::borrow::Cow;
use std::fmt::Debug;
use std::io::{self, Read};

use crate::backend::{BackendError, Signature};
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
/// [`read_current_mailmap`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mailmap(gix_mailmap::Snapshot);

/// Models a single replacement in a `.mailmap` file, containing an old email
/// and optionally an old name, and a new name and/or email to map them to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry<'a> {
    old_email: Cow<'a, str>,
    old_name: Option<Cow<'a, str>>,
    new_email: Option<Cow<'a, str>>,
    new_name: Option<Cow<'a, str>>,
}

impl Mailmap {
    /// Constructs a new, empty `Mailmap`.
    ///
    /// Equivalent to [`Default::default()`], but makes the intent more
    /// explicit.
    ///
    /// An empty `Mailmap` does not use any allocations, and an absent
    /// `.mailmap` file is semantically equivalent to an empty one, so there is
    /// usually no need to use `Option<Mailmap>`.
    pub fn empty() -> Self {
        Default::default()
    }

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

    /// Returns the canonical signature corresponding to `raw_signature`.
    /// The timestamp is left untouched. Signatures with no corresponding entry
    /// are returned as‐is.
    pub fn resolve(&self, raw_signature: &Signature) -> Signature {
        let result = self.0.try_resolve(gix_actor::SignatureRef {
            name: raw_signature.name.as_bytes().into(),
            email: raw_signature.email.as_bytes().into(),
            time: Default::default(),
        });
        match result {
            Some(canonical_signature) => Signature {
                name: String::from_utf8_lossy(&canonical_signature.name).into_owned(),
                email: String::from_utf8_lossy(&canonical_signature.email).into_owned(),
                timestamp: raw_signature.timestamp.clone(),
            },
            None => raw_signature.clone(),
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

    /// Returns an iterator over the entries.
    pub fn iter(&self) -> impl Iterator<Item = Entry> {
        self.0.iter().map(|entry| Entry {
            old_email: String::from_utf8_lossy(entry.old_email()),
            old_name: entry.old_name().map(|s| String::from_utf8_lossy(s)),
            new_email: entry.new_email().map(|s| String::from_utf8_lossy(s)),
            new_name: entry.new_name().map(|s| String::from_utf8_lossy(s)),
        })
    }
}

impl<'a> Entry<'a> {
    /// Returns the old email address to match against.
    pub fn old_email(&self) -> &str {
        &self.old_email
    }

    /// Returns the old name to match against, if present.
    pub fn old_name(&self) -> Option<&str> {
        self.old_name.as_deref()
    }

    /// Returns the canonical replacement email, if present.
    pub fn new_email(&self) -> Option<&str> {
        self.new_email.as_deref()
    }

    /// Returns the canonical replacement name, if present.
    pub fn new_name(&self) -> Option<&str> {
        self.new_name.as_deref()
    }
}

/// Reads and parses the `.mailmap` file from the working‐copy commit of the
/// specified workspace. An absent `.mailmap` is treated the same way as an
/// empty file. Parse errors when reading the file are ignored, but the rest of
/// the file will still be processed.
pub async fn read_current_mailmap(
    repo: &dyn Repo,
    workspace_id: &WorkspaceId,
) -> Result<Mailmap, BackendError> {
    let Some(commit_id) = repo.view().get_wc_commit_id(workspace_id) else {
        return Ok(Mailmap::empty());
    };
    let commit = repo.store().get_commit(commit_id)?;
    let tree = commit.tree()?;
    let path = RepoPath::from_internal_string(".mailmap");
    let value = tree.path_value(path)?;
    // We ignore symbolic links, as per `gitmailmap(5)`.
    //
    // TODO: Figure out how conflicts should be handled here.
    // TODO: Should `MaterializedTreeValue::AccessDenied` be handled somehow?
    let MaterializedTreeValue::File { mut reader, id, .. } =
        materialize_tree_value(repo.store(), path, value).await?
    else {
        return Ok(Mailmap::empty());
    };
    Mailmap::from_reader(&mut reader).map_err(|err| BackendError::ReadFile {
        path: path.to_owned(),
        id,
        source: err.into(),
    })
}
