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

//! Generic APIs to work with cryptographic signatures created and verified by
//! various backends.

use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::RwLock;

use thiserror::Error;

use crate::backend::CommitId;
use crate::gpg_signing::GpgBackend;
use crate::settings::UserSettings;
use crate::ssh_signing::SshBackend;

/// A status of the signature, part of the [Verification] type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigStatus {
    /// Valid signature that matches the data.
    Good,
    /// Valid signature that could not be verified (e.g. due to an unknown key).
    Unknown,
    /// Valid signature that does not match the signed data.
    Bad,
}

/// The result of a signature verification.
/// Key and display are optional additional info that backends can or can not
/// provide to add additional information for the templater to potentially show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verification {
    /// The status of the signature.
    pub status: SigStatus,
    /// The key id representation, if available. For GPG, this will be the key
    /// fingerprint.
    pub key: Option<String>,
    /// A display string, if available. For GPG, this will be formatted primary
    /// user ID.
    pub display: Option<String>,
}

impl Verification {
    /// A shortcut to create an `Unknown` verification with no additional
    /// metadata.
    pub fn unknown() -> Self {
        Self {
            status: SigStatus::Unknown,
            key: None,
            display: None,
        }
    }

    /// Create a new verification
    pub fn new(status: SigStatus, key: Option<String>, display: Option<String>) -> Self {
        Self {
            status,
            key,
            display,
        }
    }
}

/// The backend for signing and verifying cryptographic signatures.
///
/// This allows using different signers, such as GPG or SSH, or different
/// versions of them.
pub trait SigningBackend: Debug + Send + Sync {
    /// Name of the backend, used in the config and for display.
    fn name(&self) -> &str;

    /// Check if the signature can be read and verified by this backend.
    ///
    /// Should check the signature format, usually just looks at the prefix.
    fn can_read(&self, signature: &[u8]) -> bool;

    /// Create a signature for arbitrary data.
    ///
    /// The `key` parameter is what `jj sign` receives as key argument, or what
    /// is configured in the `signing.key` config.
    fn sign(&self, data: &[u8], key: Option<&str>) -> SignResult<Vec<u8>>;

    /// Verify a signature. Should be reflexive with `sign`:
    /// ```rust,ignore
    /// verify(data, sign(data)?)?.status == SigStatus::Good
    /// ```
    fn verify(&self, data: &[u8], signature: &[u8]) -> SignResult<Verification>;
}

/// An error type for the signing/verifying operations
#[derive(Debug, Error)]
pub enum SignError {
    /// The verification failed because the signature *format* was invalid.
    #[error("Invalid signature")]
    InvalidSignatureFormat,
    /// A generic error from the backend impl.
    #[error("Signing error")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// A result type for the signing/verifying operations
pub type SignResult<T> = Result<T, SignError>;

/// An error type for the signing backend initialization.
#[derive(Debug, Error)]
pub enum SignInitError {
    /// If the backend name specified in the config is not known.
    #[error("Unknown signing backend configured: {0}")]
    UnknownBackend(String),
    /// A generic error from the backend impl.
    #[error("Failed to initialize signing")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// A enum that describes if a created/rewritten commit should be signed or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SignBehavior {
    /// Drop existing signatures.
    /// This is what jj did before signing support or does now when a signing
    /// backend is not configured.
    #[default]
    Drop,
    /// Only sign commits that were authored by self and already signed,
    /// "preserving" the signature across rewrites.
    /// This is what jj does when a signing backend is configured.
    Keep,
    /// Sign/re-sign commits that were authored by self and drop them for
    /// others. This is what jj does when configured to always sign.
    Own,
    /// Always sign commits, regardless of who authored or signed them before.
    /// This is what jj does on `jj sign -f`.
    Force,
}

/// Wraps low-level signing backends and adds caching, similar to `Store`.
#[derive(Debug, Default)]
pub struct Signer {
    /// The backend that is used for signing commits.
    /// Optional because signing might not be configured.
    main_backend: Option<Box<dyn SigningBackend>>,
    /// All known backends without the main one - used for verification.
    /// Main backend is also used for verification, but it's not in this list
    /// for ownership reasons.
    backends: Vec<Box<dyn SigningBackend>>,
    cache: RwLock<HashMap<CommitId, Verification>>,
}

impl Signer {
    /// Creates a signer based on user settings. Uses all known backends, and
    /// chooses one of them to be used for signing depending on the config.
    pub fn from_settings(settings: &UserSettings) -> Result<Self, SignInitError> {
        let mut backends = vec![
            Box::new(GpgBackend::from_config(settings.config())) as Box<dyn SigningBackend>,
            Box::new(SshBackend::from_config(settings.config())) as Box<dyn SigningBackend>,
            // Box::new(X509Backend::from_settings(settings)?) as Box<dyn SigningBackend>,
        ];

        let main_backend = settings
            .signing_backend()
            .map(|backend| {
                backends
                    .iter()
                    .position(|b| b.name() == backend)
                    .map(|i| backends.remove(i))
                    .ok_or(SignInitError::UnknownBackend(backend))
            })
            .transpose()?;

        Ok(Self::new(main_backend, backends))
    }

    /// Creates a signer with the given backends.
    pub fn new(
        main_backend: Option<Box<dyn SigningBackend>>,
        other_backends: Vec<Box<dyn SigningBackend>>,
    ) -> Self {
        Self {
            main_backend,
            backends: other_backends,
            cache: Default::default(),
        }
    }

    /// Checks if the signer can sign, i.e. if a main backend is configured.
    pub fn can_sign(&self) -> bool {
        self.main_backend.is_some()
    }

    /// This is just a pass-through to the main backend that unconditionally
    /// creates a signature.
    pub fn sign(&self, data: &[u8], key: Option<&str>) -> SignResult<Vec<u8>> {
        self.main_backend
            .as_ref()
            .expect("tried to sign without checking can_sign first")
            .sign(data, key)
    }

    /// Looks for backend that can verify the signature and returns the result
    /// of its verification.
    pub fn verify(
        &self,
        commit_id: &CommitId,
        data: &[u8],
        signature: &[u8],
    ) -> SignResult<Verification> {
        let cached = self.cache.read().unwrap().get(commit_id).cloned();
        if let Some(check) = cached {
            return Ok(check);
        }

        let verification = self
            .main_backend
            .iter()
            .chain(self.backends.iter())
            .filter(|b| b.can_read(signature))
            // skip unknown and invalid sigs to allow other backends that can read to try
            // for example, we might have gpg and sq, both of which could read a PGP signature
            .find_map(|backend| match backend.verify(data, signature) {
                Ok(check) if check.status == SigStatus::Unknown => None,
                Err(SignError::InvalidSignatureFormat) => None,
                e => Some(e),
            })
            .transpose()?;

        if let Some(verification) = verification {
            // a key might get imported before next call?.
            // realistically this is unlikely, but technically
            // it's correct to not cache unknowns here
            if verification.status != SigStatus::Unknown {
                self.cache
                    .write()
                    .unwrap()
                    .insert(commit_id.clone(), verification.clone());
            }
            Ok(verification)
        } else {
            // now here it's correct to cache unknowns, as we don't
            // have a backend that knows how to handle this signature
            //
            // not sure about how much of an optimization this is
            self.cache
                .write()
                .unwrap()
                .insert(commit_id.clone(), Verification::unknown());
            Ok(Verification::unknown())
        }
    }
}
