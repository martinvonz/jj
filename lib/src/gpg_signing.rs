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

#![allow(missing_docs)]

use std::ffi::{OsStr, OsString};
use std::fmt::Debug;
use std::io::Write;
use std::process::{Command, ExitStatus, Stdio};

use thiserror::Error;

use crate::signing::{SigStatus, SignError, SigningBackend, Verification};

#[derive(Debug)]
pub struct GpgBackend {
    program: OsString,
    consider_expired_keys_bad: bool,
    extra_args: Vec<OsString>,
}

#[derive(Debug, Error)]
pub enum GpgError {
    #[error("GPG failed with exit status {exit_status}:\n{stderr}")]
    Command {
        exit_status: ExitStatus,
        stderr: String,
    },
    #[error("Failed to run GPG: {0}")]
    Io(#[from] std::io::Error),
}

impl From<GpgError> for SignError {
    fn from(e: GpgError) -> Self {
        SignError::Backend(Box::new(e))
    }
}

impl GpgBackend {
    pub fn new(program: OsString, consider_expired_keys_bad: bool) -> Self {
        Self {
            program,
            consider_expired_keys_bad,
            extra_args: vec![],
        }
    }

    /// Primarily intended for testing
    pub fn with_extra_args(mut self, args: &[OsString]) -> Self {
        self.extra_args.extend_from_slice(args);
        self
    }

    pub fn from_config(config: &config::Config) -> Self {
        Self::new(
            config
                .get_string("signing.backends.GPG.program")
                .unwrap_or_else(|_| "gpg2".into())
                .into(),
            config
                .get_bool("signing.backends.GPG.consider-expired-keys-bad")
                .unwrap_or(false),
        )
    }

    fn run(&self, input: &[u8], args: &[&OsStr], check: bool) -> Result<Vec<u8>, GpgError> {
        let process = Command::new(&self.program)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(if check { Stdio::piped() } else { Stdio::null() })
            .args(&self.extra_args)
            .args(args)
            .spawn()?;
        process.stdin.as_ref().unwrap().write_all(input)?;
        let output = process.wait_with_output()?;
        if check && !output.status.success() {
            Err(GpgError::Command {
                exit_status: output.status,
                stderr: String::from_utf8_lossy(&output.stderr).into(),
            })
        } else {
            Ok(output.stdout)
        }
    }
}

impl SigningBackend for GpgBackend {
    fn name(&self) -> &str {
        "GPG"
    }

    fn can_read(&self, signature: &[u8]) -> bool {
        signature.starts_with(b"-----BEGIN PGP SIGNATURE-----")
    }

    fn sign(&self, data: &[u8], key: Option<&str>) -> Result<Vec<u8>, SignError> {
        Ok(match key {
            Some(key) => self.run(data, &["-abu".as_ref(), key.as_ref()], true)?,
            None => self.run(data, &["-ab".as_ref()], true)?,
        })
    }

    fn verify(&self, data: &[u8], signature: &[u8]) -> Result<Verification, SignError> {
        let mut signature_file = tempfile::Builder::new()
            .prefix(".jj-gpg-sig-tmp-")
            .tempfile()
            .map_err(GpgError::Io)?;
        signature_file.write_all(signature).map_err(GpgError::Io)?;
        signature_file.flush().map_err(GpgError::Io)?;

        let output = self.run(
            data,
            &[
                "--status-fd=1".as_ref(),
                "--verify".as_ref(),
                signature_file.path().as_os_str(), /* the only reason we have those .as_refs
                                                    * transmuting to &OsStr everywhere.. */
                "-".as_ref(),
            ],
            false,
        )?;

        // Search for one of the:
        //  [GNUPG:] GOODSIG <long keyid> <primary uid..>
        //  [GNUPG:] EXPKEYSIG <long keyid> <primary uid..>
        //  [GNUPG:] NO_PUBKEY <long keyid>
        //  [GNUPG:] BADSIG <long keyid> <primary uid..>
        // in the output from --status-fd=1
        // Assume signature is invalid if none of the above was found
        output
            .split(|&b| b == b'\n')
            .filter_map(|line| line.strip_prefix(b"[GNUPG:] "))
            .find_map(|line| {
                line.strip_prefix(b"GOODSIG ")
                    .map(|rest| (SigStatus::Good, rest))
                    .or_else(|| {
                        line.strip_prefix(b"EXPKEYSIG ").map(|rest| {
                            let status = if self.consider_expired_keys_bad {
                                SigStatus::Bad
                            } else {
                                SigStatus::Good
                            };
                            (status, rest)
                        })
                    })
                    .or_else(|| {
                        line.strip_prefix(b"NO_PUBKEY ")
                            .map(|rest| (SigStatus::Unknown, rest))
                    })
                    .or_else(|| {
                        line.strip_prefix(b"BADSIG ")
                            .map(|rest| (SigStatus::Bad, rest))
                    })
            })
            .map(|(status, line)| {
                let mut parts = line.splitn(2, |&b| b == b' ');
                let key = parts
                    .next()
                    .and_then(|bs| String::from_utf8(bs.to_owned()).ok());
                let display = parts
                    .next()
                    .and_then(|bs| String::from_utf8(bs.to_owned()).ok());
                Verification {
                    status,
                    key,
                    display,
                }
            })
            .ok_or(SignError::InvalidSignatureFormat)
    }
}
