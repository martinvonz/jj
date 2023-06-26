use std::collections::HashMap;
use std::fmt::Debug;
use std::iter;
use std::sync::{Arc, RwLock};

use thiserror::Error;

use crate::backend::{CommitId, Sig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationStatus {
    /// Valid signature that matches the data.
    Good,
    /// Valid signature that could not be verified (e.g. due to an unknown key).
    Unknown,
    /// Valid signature that does not match the signed data.
    Bad,
    /// Invalid signature.
    Invalid,
}

#[derive(Debug, Clone)]
pub struct Verification {
    pub status: VerificationStatus,
    pub key: Option<String>,
    pub display: Option<String>,
}

impl Verification {
    pub fn unknown() -> Self {
        Self {
            status: VerificationStatus::Unknown,
            key: None,
            display: None,
        }
    }
    pub fn invalid() -> Self {
        Self {
            status: VerificationStatus::Invalid,
            key: None,
            display: None,
        }
    }
}

/// The backend for signing and verifying cryptographic signatures.
///
/// This allows using different signers, such as GPG or SSH, or different
/// versions of them.
///
/// The instance of the backend carries the principal that is used to make
/// signatures, e.g. for GPG it's a key id (and a GPG-managed key itself).
pub trait SigningBackend: Debug + Send + Sync {
    /// Update the principal that the backend should use.
    ///
    /// The parameter is what `jj sign` receives as key argument,
    fn update_key(&self, key: Option<String>);

    /// Check if the signature was created by this backend implementation.
    ///
    /// Should check the signature format, usually just looks at the prefix.
    fn is_of_this_type(&self, signature: &[u8]) -> bool;

    /// Check if the signature was created by this backend instance.
    ///
    /// Should check if e.g. the key fingerprint of the signature matches the
    /// one of the current signer.
    fn is_own(&self, signature: &[u8]) -> Result<bool, SignError>;

    /// Create a signature for arbitrary data.
    fn sign(&self, data: &[u8]) -> Result<Vec<u8>, SignError>;

    /// Verify a signature. Should be reflexive with `sign`:
    /// ```rust,ignore
    /// verify(data, sign(data)?) == Ok(SigCheck::Good)
    /// ```
    fn verify(&self, data: &[u8], signature: &[u8]) -> Result<Verification, SignError>;
}

#[derive(Debug, Error)]
pub enum SignError {
    #[error("Cannot re-sign a commit that was already signed")]
    CannotRewrite,
    #[error("Signing error: {0}")]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SignConfig {
    /// Drop existing signatures (what jj did before signing support)
    #[default]
    Drop,
    /// Only sign commits that were already signed, fails if the signature was
    /// foreign
    Own,
    /// Sign commits that are not signed or were signed by self, fails if the
    /// signature was foreign
    New,
    /// Re-sign commits that were already signed, regardless of who signed them
    ReSign,
}

impl SignConfig {
    pub fn setting(enabled: bool) -> Self {
        if enabled {
            Self::New
        } else {
            Self::Drop
        }
    }

    pub fn rebase(enabled: bool) -> Self {
        if enabled {
            Self::Own
        } else {
            Self::Drop
        }
    }
}

#[derive(Debug)]
pub struct Signer {
    main_backend: Box<dyn SigningBackend>,
    other_backends: Vec<Box<dyn SigningBackend>>,
    config: RwLock<SignConfig>,
    enabled: RwLock<bool>,
    own_cache: RwLock<HashMap<CommitId, bool>>,
    verification_cache: RwLock<HashMap<CommitId, Verification>>,
}

impl Signer {
    pub fn new(
        main_backend: Box<dyn SigningBackend>,
        other_backends: Vec<Box<dyn SigningBackend>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            main_backend,
            other_backends,
            config: Default::default(),
            enabled: Default::default(),
            own_cache: Default::default(),
            verification_cache: Default::default(),
        })
    }

    pub fn enable(&self, key: Option<String>) {
        *self.enabled.write().unwrap() = true;
        self.main_backend.update_key(key);
    }

    pub fn is_enabled(&self) -> bool {
        *self.enabled.read().unwrap()
    }

    pub fn config(&self, config: SignConfig) {
        *self.config.write().unwrap() = config;
    }

    /// There are several returns this function can have:
    /// - Ok(true) -> proceed with signing
    /// - Ok(false) -> do not sign - for rewrites this means dropping the
    ///   signature if it was there
    /// - Err(SigningError::CannotRewrite) -> abort the commit as the signature
    ///   cannot be created nor dropped, e.g. because we don't want to rewrite a
    ///   foreign signature
    /// - Err(_) -> downstream errors from signing backend, this return is
    ///   listed for completeness
    pub fn will_sign(&self, existing: Option<&Sig>) -> Result<bool, SignError> {
        match *self.config.read().unwrap() {
            SignConfig::Drop => Ok(false),
            c @ (SignConfig::Own | SignConfig::New) => {
                if let Some(sig) = existing {
                    if self.main_backend.is_own(&sig.signature)? {
                        Ok(true)
                    } else {
                        Err(SignError::CannotRewrite)
                    }
                } else {
                    Ok(c == SignConfig::New)
                }
            }
            SignConfig::ReSign => Ok(existing.is_some()),
        }
    }

    /// This is just a pass-through to the main backend that unconditionally
    /// creates a signature.
    pub fn sign(&self, data: &[u8]) -> Result<Vec<u8>, SignError> {
        self.main_backend.sign(data)
    }

    pub fn is_own(&self, commit_id: &CommitId, signature: &[u8]) -> Result<bool, SignError> {
        if let Some(cached) = self.own_cache.read().unwrap().get(commit_id) {
            return Ok(*cached);
        }
        let check = self.main_backend.is_own(signature)?;
        self.own_cache
            .write()
            .unwrap()
            .insert(commit_id.clone(), check);
        Ok(check)
    }

    pub fn verify(
        &self,
        commit_id: &CommitId,
        data: &[u8],
        signature: &[u8],
    ) -> Result<Verification, SignError> {
        let cached = self
            .verification_cache
            .read()
            .unwrap()
            .get(commit_id)
            .cloned();
        if let Some(check) = cached {
            return Ok(check);
        }
        if let Some(backend) = iter::once(&self.main_backend)
            .chain(self.other_backends.iter())
            .find(|b| b.is_of_this_type(signature))
        {
            let check = backend.verify(data, signature)?;

            // a key might get imported before next call?.
            // realistically this is unlikely, but technically
            // it's correct to not cache unknowns here
            if check.status != VerificationStatus::Unknown {
                self.verification_cache
                    .write()
                    .unwrap()
                    .insert(commit_id.clone(), check.clone());
            }
            Ok(check)
        } else {
            // now here it's correct to cache unknowns, as we don't
            // have a backend that knows how to handle this signature
            //
            // not sure about how much of an optimization this is
            self.verification_cache
                .write()
                .unwrap()
                .insert(commit_id.clone(), Verification::unknown());
            Ok(Verification::unknown())
        }
    }
}
