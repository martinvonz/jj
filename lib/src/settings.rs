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

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::DateTime;
use rand::prelude::*;
use rand_chacha::ChaCha20Rng;

use crate::backend::{ChangeId, ObjectId, Signature, Timestamp};
use crate::gpg_signer::GpgSigner;
use crate::signing::Signer;

#[derive(Debug, Clone)]
pub struct UserSettings {
    config: config::Config,
    timestamp: Option<Timestamp>,
    rng: Arc<JJRng>,
    signer: Arc<Signer>,
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
    config
        .get_string(key)
        .ok()
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(Timestamp::from_datetime)
}

fn get_rng_seed_config(config: &config::Config) -> Option<u64> {
    config
        .get_string("debug.randomness-seed")
        .ok()
        .and_then(|str| str.parse().ok())
}

fn get_signer(config: &config::Config) -> Arc<Signer> {
    let enabled = config.get_bool("sign.enabled").unwrap_or_default();

    // a little overkill but eh
    let mut backends = HashMap::from([
        ("gpg", Box::new(GpgSigner::from_config(config)) as _),
        // (
        //     "ssh",
        //     Box::new(SshSigner::new(key, SshSignerSettings::from_config(config))) as _,
        // ),
    ]);

    let selected = config
        .get_string("sign.backend")
        .map_or(Cow::Borrowed("gpg"), Cow::Owned); // heh

    let backend = backends
        .remove(&*selected)
        .unwrap_or_else(|| backends.remove("gpg").unwrap());
    let signer = Signer::new(backend, backends.into_values().collect());
    if enabled {
        signer.enable(config.get_string("sign.key").ok());
    }
    signer
}

impl UserSettings {
    pub fn from_config(config: config::Config) -> Self {
        let timestamp = get_timestamp_config(&config, "debug.commit-timestamp");
        let rng_seed = get_rng_seed_config(&config);
        let signer = get_signer(&config);

        UserSettings {
            config,
            timestamp,
            rng: Arc::new(JJRng::new(rng_seed)),
            signer,
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
        self.config.get_string("revsets.log").unwrap_or_else(|_| {
            // For compatibility with old config files (<0.8.0)
            self.config
                .get_string("ui.default-revset")
                .unwrap_or_else(|_| {
                    "@ | (remote_branches() | tags()).. | ((remote_branches() | tags())..)-"
                        .to_string()
                })
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

    pub fn signer(&self) -> Arc<Signer> {
        self.signer.clone()
    }

    pub fn sign_key(&self) -> Option<String> {
        self.config.get_string("sign.key").ok()
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
