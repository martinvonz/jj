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

use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::DateTime;
use rand::prelude::*;
use rand_chacha::ChaCha20Rng;

use crate::backend::{ChangeId, ObjectId, Signature, Timestamp, CHANGE_ID_HASH_LENGTH};

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

    pub fn user_name(&self) -> String {
        self.config
            .get_string("user.name")
            .unwrap_or_else(|_| Self::user_name_placeholder().to_string())
    }

    pub fn user_name_placeholder() -> &'static str {
        "(no name configured)"
    }

    pub fn user_email(&self) -> String {
        self.config
            .get_string("user.email")
            .unwrap_or_else(|_| Self::user_email_placeholder().to_string())
    }

    pub fn user_email_placeholder() -> &'static str {
        "(no email configured)"
    }

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
            .get_string("push.branch-prefix")
            .unwrap_or_else(|_| "push-".to_string())
    }

    pub fn default_revset(&self) -> String {
        self.config
            .get_string("ui.default-revset")
            .unwrap_or_else(|_| {
                "@ | (remote_branches() | tags()).. | ((remote_branches() | tags())..)-".to_string()
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

    pub fn relative_timestamps(&self) -> bool {
        self.config
            .get_bool("ui.relative-timestamps")
            .unwrap_or(false)
    }

    pub fn unique_prefixes(&self) -> String {
        self.config
            .get_string("ui.unique-prefixes")
            .unwrap_or_else(|_| "brackets".to_string())
    }

    pub fn config(&self) -> &config::Config {
        &self.config
    }

    pub fn graph_format(&self) -> String {
        self.config
            .get_string("ui.graph.format")
            .unwrap_or_else(|_| "ascii".to_string())
    }
}

/// This Rng uses interior mutability to allow generating random values using an
/// immutable reference. It also fixes a specific seedable RNG for
/// reproducibility.
#[derive(Debug)]
pub struct JJRng(Mutex<ChaCha20Rng>);
impl JJRng {
    pub fn new_change_id(&self) -> ChangeId {
        let random_bytes: [u8; CHANGE_ID_HASH_LENGTH] = self.gen();
        ChangeId::new(random_bytes.into())
    }

    /// Wraps Rng::gen but only requires an immutable reference. Can be made
    /// public if there's a use for it.
    fn gen<T>(&self) -> T
    where
        rand::distributions::Standard: rand::distributions::Distribution<T>,
    {
        let mut rng = self.0.lock().unwrap();
        rng.gen()
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
