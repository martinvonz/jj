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

use hex::ToHex;
use jj_lib::content_hash::blake2b_hash;
use jj_lib::signing::{SigStatus, SignError, SignResult, SigningBackend, Verification};

#[derive(Debug)]
#[allow(missing_docs)]
pub struct MockSigningBackend;

const PREFIX: &str = "--- JJ-MOCK-SIGNATURE ---\nKEY: ";

impl SigningBackend for MockSigningBackend {
    fn name(&self) -> &str {
        "mock"
    }

    fn can_read(&self, signature: &[u8]) -> bool {
        signature.starts_with(PREFIX.as_bytes())
    }

    fn sign(&self, data: &[u8], key: Option<&str>) -> SignResult<Vec<u8>> {
        let key = key.unwrap_or_default();
        let mut body = Vec::with_capacity(data.len() + key.len());
        body.extend_from_slice(key.as_bytes());
        body.extend_from_slice(data);

        let hash: String = blake2b_hash(&body).encode_hex();

        Ok(format!("{PREFIX}{key}\n{hash}").into_bytes())
    }

    fn verify(&self, data: &[u8], signature: &[u8]) -> SignResult<Verification> {
        let Some(key) = signature
            .strip_prefix(PREFIX.as_bytes())
            .and_then(|s| s.splitn(2, |&b| b == b'\n').next())
        else {
            return Err(SignError::InvalidSignatureFormat);
        };
        let key = (!key.is_empty()).then_some(std::str::from_utf8(key).unwrap().to_owned());

        let sig = self.sign(data, key.as_deref())?;
        let status = if sig == signature {
            SigStatus::Good
        } else {
            SigStatus::Bad
        };
        Ok(Verification::new(status, key, None))
    }
}
