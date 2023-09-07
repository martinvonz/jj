// Copyright 2020 The Jujutsu Authors
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

#![allow(missing_docs)]

use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::DateTime;
use rand::prelude::*;
use rand_chacha::ChaCha20Rng;

use crate::backend::{ChangeId, ObjectId, Signature, Timestamp};
use crate::fmt_util::binary_prefix;
use crate::fsmonitor::FsmonitorKind;

#[derive(Debug, Clone)]
pub struct UserSettings {
    config: config::Config,
    timestamp: Option<Timestamp>,
    rng: Arc<JJRng>,
}

#[derive(Debug, Clone)]
pub struct RepoSettings {
    _config: config::Config,
}

#[derive(Debug, Clone)]
pub struct GitSettings {
    pub auto_local_branch: bool,
}

impl GitSettings {
    pub fn from_config(config: &config::Config) -> Self {
        GitSettings {
            auto_local_branch: config.get_bool("git.auto-local-branch").unwrap_or(true),
        }
    }
}

impl Default for GitSettings {
    fn default() -> Self {
        GitSettings {
            auto_local_branch: true,
        }
    }
}

fn get_timestamp_config(config: &config::Config, key: &str) -> Option<Timestamp> {
    match config.get_string(key) {
        Ok(timestamp_str) => match DateTime::parse_from_rfc3339(&timestamp_str) {
            Ok(datetime) => Some(Timestamp::from_datetime(datetime)),
            Err(_) => None,
        },
        Err(_) => None,
    }
}

fn get_rng_seed_config(config: &config::Config) -> Option<u64> {
    config
        .get_string("debug.randomness-seed")
        .ok()
        .and_then(|str| str.parse().ok())
}

impl UserSettings {
    pub fn from_config(config: config::Config) -> Self {
        let timestamp = get_timestamp_config(&config, "debug.commit-timestamp");
        let rng_seed = get_rng_seed_config(&config);
        UserSettings {
            config,
            timestamp,
            rng: Arc::new(JJRng::new(rng_seed)),
        }
    }

    // TODO: Reconsider UserSettings/RepoSettings abstraction. See
    // https://github.com/martinvonz/jj/issues/616#issuecomment-1345170699
    pub fn with_repo(&self, _repo_path: &Path) -> Result<RepoSettings, config::ConfigError> {
        let config = self.config.clone();
        Ok(RepoSettings { _config: config })
    }

    pub fn get_rng(&self) -> Arc<JJRng> {
        self.rng.clone()
    }

    pub fn use_tree_conflict_format(&self) -> bool {
        self.config
            .get_bool("format.tree-level-conflicts")
            .unwrap_or(false)
    }

    pub fn user_name(&self) -> String {
        self.config.get_string("user.name").unwrap_or_default()
    }

    // Must not be changed to avoid git pushing older commits with no set name
    pub const USER_NAME_PLACEHOLDER: &'static str = "(no name configured)";

    pub fn user_email(&self) -> String {
        self.config.get_string("user.email").unwrap_or_default()
    }

    pub fn fsmonitor_kind(&self) -> Result<Option<FsmonitorKind>, config::ConfigError> {
        match self.config.get_string("core.fsmonitor") {
            Ok(fsmonitor_kind) => Ok(Some(fsmonitor_kind.parse()?)),
            Err(config::ConfigError::NotFound(_)) => Ok(None),
            Err(err) => Err(err),
        }
    }

    // Must not be changed to avoid git pushing older commits with no set email
    // address
    pub const USER_EMAIL_PLACEHOLDER: &'static str = "(no email configured)";

    pub fn operation_timestamp(&self) -> Option<Timestamp> {
        get_timestamp_config(&self.config, "debug.operation-timestamp")
    }

    pub fn operation_hostname(&self) -> String {
        self.config
            .get_string("operation.hostname")
            .unwrap_or_else(|_| whoami::hostname())
    }

    pub fn operation_username(&self) -> String {
        self.config
            .get_string("operation.username")
            .unwrap_or_else(|_| whoami::username())
    }

    pub fn push_branch_prefix(&self) -> String {
        self.config
            .get_string("git.push-branch-prefix")
            .unwrap_or_else(|_| "push-".to_string())
    }

    pub fn default_description(&self) -> String {
        self.config()
            .get_string("ui.default-description")
            .unwrap_or_default()
    }

    pub fn default_revset(&self) -> String {
        self.config.get_string("revsets.log").unwrap_or_else(|_| {
            // For compatibility with old config files (<0.8.0)
            self.config
                .get_string("ui.default-revset")
                .unwrap_or_else(|_| "@ | ancestors((remote_branches() | tags()).., 2)".to_string())
        })
    }

    pub fn signature(&self) -> Signature {
        let timestamp = self.timestamp.clone().unwrap_or_else(Timestamp::now);
        Signature {
            name: self.user_name(),
            email: self.user_email(),
            timestamp,
        }
    }

    pub fn allow_native_backend(&self) -> bool {
        self.config
            .get_bool("ui.allow-init-native")
            .unwrap_or(false)
    }

    pub fn diff_instructions(&self) -> bool {
        self.config.get_bool("ui.diff-instructions").unwrap_or(true)
    }

    pub fn config(&self) -> &config::Config {
        &self.config
    }

    pub fn git_settings(&self) -> GitSettings {
        GitSettings::from_config(&self.config)
    }

    pub fn graph_style(&self) -> String {
        self.config
            .get_string("ui.graph.style")
            .unwrap_or_else(|_| "curved".to_string())
    }

    pub fn max_new_file_size(&self) -> Result<u64, config::ConfigError> {
        let cfg = self
            .config
            .get::<HumanByteSize>("snapshot.max-new-file-size")
            .map(|x| x.0);
        match cfg {
            Ok(0) => Ok(u64::MAX),
            x @ Ok(_) => x,
            Err(config::ConfigError::NotFound(_)) => Ok(1024 * 1024),
            e @ Err(_) => e,
        }
    }
}

/// This Rng uses interior mutability to allow generating random values using an
/// immutable reference. It also fixes a specific seedable RNG for
/// reproducibility.
#[derive(Debug)]
pub struct JJRng(Mutex<ChaCha20Rng>);
impl JJRng {
    pub fn new_change_id(&self, length: usize) -> ChangeId {
        let mut rng = self.0.lock().unwrap();
        let random_bytes = (0..length).map(|_| rng.gen::<u8>()).collect();
        ChangeId::new(random_bytes)
    }

    /// Creates a new RNGs. Could be made public, but we'd like to encourage all
    /// RNGs references to point to the same RNG.
    fn new(seed: Option<u64>) -> Self {
        Self(Mutex::new(JJRng::internal_rng_from_seed(seed)))
    }

    fn internal_rng_from_seed(seed: Option<u64>) -> ChaCha20Rng {
        match seed {
            Some(seed) => ChaCha20Rng::seed_from_u64(seed),
            None => ChaCha20Rng::from_entropy(),
        }
    }
}

pub trait ConfigResultExt<T> {
    fn optional(self) -> Result<Option<T>, config::ConfigError>;
}

impl<T> ConfigResultExt<T> for Result<T, config::ConfigError> {
    fn optional(self) -> Result<Option<T>, config::ConfigError> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(config::ConfigError::NotFound(_)) => Ok(None),
            Err(err) => Err(err),
        }
    }
}

/// A size in bytes optionally formatted/serialized with binary prefixes
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct HumanByteSize(pub u64);

impl std::fmt::Display for HumanByteSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (value, prefix) = binary_prefix(self.0 as f32);
        write!(f, "{value:.1}{prefix}B")
    }
}

impl<'de> serde::Deserialize<'de> for HumanByteSize {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;

        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = HumanByteSize;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(formatter, "a size in bytes with an optional binary unit")
            }

            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
            where
                E: Error,
            {
                Ok(HumanByteSize(v))
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: Error,
            {
                let bytes = parse_human_byte_size(v).map_err(Error::custom)?;
                Ok(HumanByteSize(bytes))
            }
        }

        if deserializer.is_human_readable() {
            deserializer.deserialize_any(Visitor)
        } else {
            deserializer.deserialize_u64(Visitor)
        }
    }
}

fn parse_human_byte_size(v: &str) -> Result<u64, &str> {
    let digit_end = v.find(|c: char| !c.is_ascii_digit()).unwrap_or(v.len());
    if digit_end == 0 {
        return Err("must start with a number");
    }
    let (digits, trailing) = v.split_at(digit_end);
    let exponent = match trailing.trim_start() {
        "" | "B" => 0,
        unit => {
            const PREFIXES: [char; 8] = ['K', 'M', 'G', 'T', 'P', 'E', 'Z', 'Y'];
            let Some(prefix) = PREFIXES.iter().position(|&x| unit.starts_with(x)) else {
                return Err("unrecognized unit prefix");
            };
            let ("" | "B" | "i" | "iB") = &unit[1..] else {
                return Err("unrecognized unit");
            };
            prefix as u32 + 1
        }
    };
    // A string consisting only of base 10 digits is either a valid u64 or really
    // huge.
    let factor = digits.parse::<u64>().unwrap_or(u64::MAX);
    Ok(factor.saturating_mul(1024u64.saturating_pow(exponent)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_size_parse() {
        assert_eq!(parse_human_byte_size("0"), Ok(0));
        assert_eq!(parse_human_byte_size("42"), Ok(42));
        assert_eq!(parse_human_byte_size("42B"), Ok(42));
        assert_eq!(parse_human_byte_size("42 B"), Ok(42));
        assert_eq!(parse_human_byte_size("42K"), Ok(42 * 1024));
        assert_eq!(parse_human_byte_size("42 K"), Ok(42 * 1024));
        assert_eq!(parse_human_byte_size("42 KB"), Ok(42 * 1024));
        assert_eq!(parse_human_byte_size("42 KiB"), Ok(42 * 1024));
        assert_eq!(
            parse_human_byte_size("42 LiB"),
            Err("unrecognized unit prefix")
        );
        assert_eq!(parse_human_byte_size("42 KiC"), Err("unrecognized unit"));
        assert_eq!(parse_human_byte_size("42 KC"), Err("unrecognized unit"));
        assert_eq!(
            parse_human_byte_size("KiB"),
            Err("must start with a number")
        );
        assert_eq!(parse_human_byte_size(""), Err("must start with a number"));
    }
}
