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

// TODO: remove this file after watchman adopts and releases it.
// https://github.com/facebook/watchman/pull/1221
#![allow(missing_docs)]

use std::path::PathBuf;

use serde::{Deserialize, Serialize, Serializer};
use watchman_client::expr::Expr;
use watchman_client::{Client, Error, ResolvedRoot};

/// Registers a trigger.
pub async fn register_trigger(
    client: &Client,
    root: &ResolvedRoot,
    request: TriggerRequest,
) -> Result<TriggerResponse, Error> {
    let response: TriggerResponse = client
        .generic_request(TriggerCommand(
            "trigger",
            root.project_root().to_path_buf(),
            request,
        ))
        .await?;
    Ok(response)
}

/// Removes a registered trigger.
pub async fn remove_trigger(
    client: &Client,
    root: &ResolvedRoot,
    name: &str,
) -> Result<TriggerDelResponse, Error> {
    let response: TriggerDelResponse = client
        .generic_request(TriggerDelCommand(
            "trigger-del",
            root.project_root().to_path_buf(),
            name.into(),
        ))
        .await?;
    Ok(response)
}

/// Lists registered triggers.
pub async fn list_triggers(
    client: &Client,
    root: &ResolvedRoot,
) -> Result<TriggerListResponse, Error> {
    let response: TriggerListResponse = client
        .generic_request(TriggerListCommand(
            "trigger-list",
            root.project_root().to_path_buf(),
        ))
        .await?;
    Ok(response)
}

/// The `trigger` command request.
///
/// The fields are explained in detail here:
/// <https://facebook.github.io/watchman/docs/cmd/trigger#extended-syntax>
#[derive(Deserialize, Serialize, Default, Clone, Debug)]
pub struct TriggerRequest {
    /// Defines the name of the trigger.
    pub name: String,

    /// Specifies the command to invoke.
    pub command: Vec<String>,

    /// It true, matching files (up to system limits) will be added to the
    /// command's execution args.
    #[serde(default, skip_serializing_if = "is_false")]
    pub append_files: bool,

    /// Specifies the expression used to filter candidate matches.
    #[serde(skip_serializing_if = "Option::is_none", skip_deserializing)]
    pub expression: Option<Expr>,

    /// Configure the way `stdin` is configured for the executed trigger.
    #[serde(
        default,
        skip_serializing_if = "TriggerStdinConfig::is_devnull",
        serialize_with = "TriggerStdinConfig::serialize",
        skip_deserializing
    )]
    pub stdin: TriggerStdinConfig,

    /// Specifies a file to write the output stream to.  Prefix with `>` to
    /// overwrite and `>>` to append.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,

    /// Specifies a file to write the error stream to.  Prefix with `>` to
    /// overwrite and `>>` to append.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,

    /// Specifies a limit on the number of files reported on stdin when stdin is
    /// set to hold the set of matched files.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_files_stdin: Option<u64>,

    /// Specifies the working directory that will be set prior to spawning the
    /// process. The default is to set the working directory to the watched
    /// root. The value of this property is a string that will be interpreted
    /// relative to the watched root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chdir: Option<String>,
}

#[derive(Clone, Debug)]
pub enum TriggerStdinConfig {
    DevNull,
    FieldNames(Vec<String>),
    NamePerLine,
}

impl Default for TriggerStdinConfig {
    fn default() -> Self {
        Self::DevNull
    }
}

impl TriggerStdinConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::DevNull => serializer.serialize_str("/dev/null"),
            Self::FieldNames(names) => serializer.collect_seq(names.iter()),
            Self::NamePerLine => serializer.serialize_str("NAME_PER_LINE"),
        }
    }

    fn is_devnull(&self) -> bool {
        matches!(self, Self::DevNull)
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct TriggerCommand(pub &'static str, pub PathBuf, pub TriggerRequest);

#[derive(Deserialize, Debug)]
pub struct TriggerResponse {
    pub version: String,
    pub disposition: String,
    pub triggerid: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct TriggerDelCommand(pub &'static str, pub PathBuf, pub String);

#[derive(Deserialize, Debug)]
pub struct TriggerDelResponse {
    pub version: String,
    pub deleted: bool,
    pub trigger: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct TriggerListCommand(pub &'static str, pub PathBuf);

#[derive(Deserialize, Debug)]
pub struct TriggerListResponse {
    pub version: String,
    pub triggers: Vec<TriggerRequest>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(v: &bool) -> bool {
    !*v
}
