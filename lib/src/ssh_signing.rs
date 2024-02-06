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
pub struct SshBackend {
    program: OsString,
    allowed_signers: Option<OsString>,
}

#[derive(Debug, Error)]
pub enum SshError {
    #[error("SSH sign failed with exit status {exit_status}:\n{stderr}")]
    Command {
        exit_status: ExitStatus,
        stderr: String,
    },
    #[error("Failed to run ssh-keygen: {0}")]
    Io(#[from] std::io::Error),
    #[error("Signing key required")]
    MissingKey {},
    #[error("Allowed signers file not provided")]
    MissingAllowedSigners {},
}

impl From<SshError> for SignError {
    fn from(e: SshError) -> Self {
        SignError::Backend(Box::new(e))
    }
}

impl SshBackend {
    pub fn new(program: OsString, allowed_signers: Option<OsString>) -> Self {
        Self {
            program,
            allowed_signers,
        }
    }

    pub fn from_config(config: &config::Config) -> Self {
        Self::new(
            config
                .get_string("signing.backends.ssh.program")
                .unwrap_or_else(|_| "ssh-keygen".into())
                .into(),
            config
                .get_string("signing.backends.ssh.allowed-signers")
                .map_or_else(|_| None, |value| Some(value.into())),
        )
    }

    fn run(&self, input: &[u8], args: &[&OsStr]) -> Result<Vec<u8>, SshError> {
        let process = Command::new(&self.program)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .args(args)
            .arg("-n")
            .arg("git")
            .spawn()?;

        process.stdin.as_ref().unwrap().write_all(input)?;
        let output = process.wait_with_output()?;

        if !output.status.success() {
            Err(SshError::Command {
                exit_status: output.status,
                stderr: String::from_utf8_lossy(&output.stderr).into(),
            })
        } else {
            Ok(output.stdout)
        }
    }

    fn get_allowed_signers(&self) -> Result<OsString, SshError> {
        if let Some(allowed_signers) = &self.allowed_signers {
            Ok(allowed_signers.into())
        } else {
            Err(SshError::MissingAllowedSigners {})
        }
    }

    fn find_principal(&self, signature_file_path: &OsStr) -> Result<Option<String>, SshError> {
        let allowed_signers = self.get_allowed_signers()?;

        let process = Command::new(&self.program)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .args([
                "-Y".as_ref(),
                "find-principals".as_ref(),
                "-f".as_ref(),
                allowed_signers.as_os_str(),
                "-s".as_ref(),
                signature_file_path,
            ])
            .spawn()?;

        let output = process.wait_with_output()?;

        let result: String = String::from_utf8_lossy(&output.stdout).into();

        let principal = result.split('\n').next().unwrap().trim().to_string();

        if principal.is_empty() {
            return Ok(None);
        }
        Ok(Some(principal))
    }
}

impl SigningBackend for SshBackend {
    fn name(&self) -> &str {
        "ssh"
    }

    fn can_read(&self, signature: &[u8]) -> bool {
        let has_signers = match self.allowed_signers {
            Some(_) => true,
            None => false,
        };
        signature.starts_with(b"-----BEGIN SSH SIGNATURE-----") && has_signers
    }

    fn sign(&self, data: &[u8], key: Option<&str>) -> Result<Vec<u8>, SignError> {
        if let Some(key) = key {
            // The ssh-keygen `-f` flag expects to be given a file which contains either a private or
            // public key.
            //
            // As it expects a file and we will generally have just public key data we need to put it into a
            // file first.
            let mut pub_key_file = tempfile::Builder::new()
                .prefix("jj-signing-pub-key-")
                .tempfile()
                .map_err(SshError::Io)?;

            pub_key_file
                .write_all(key.as_bytes())
                .map_err(SshError::Io)?;
            pub_key_file.flush().map_err(SshError::Io)?;

            let result = self.run(
                data,
                &[
                    "-Y".as_ref(),
                    "sign".as_ref(),
                    "-f".as_ref(),
                    pub_key_file.path().as_os_str(),
                ],
            );

            Ok(result?)
        } else {
            Err(SshError::MissingKey {}.into())
        }
    }

    fn verify(&self, data: &[u8], signature: &[u8]) -> Result<Verification, SignError> {
        let mut signature_file = tempfile::Builder::new()
            .prefix(".jj-ssh-sig-")
            .tempfile()
            .map_err(SshError::Io)?;
        signature_file.write_all(signature).map_err(SshError::Io)?;
        signature_file.flush().map_err(SshError::Io)?;

        let signature_file_path = signature_file.path().as_os_str();

        let principal = self.find_principal(signature_file_path)?;

        if let None = principal {
            let output = self.run(
                data,
                &[
                    "-Y".as_ref(),
                    "check-novalidate".as_ref(),
                    "-s".as_ref(),
                    signature_file_path,
                ],
            );

            return match output {
                Ok(_) => Ok(Verification::new(
                    SigStatus::Unknown,
                    None,
                    Some("Signature OK. Unknown principal".into()),
                )),
                Err(_) => Ok(Verification::new(SigStatus::Bad, None, None)),
            };
        }

        match principal {
            None => {
                let output = self.run(
                    data,
                    &[
                        "-Y".as_ref(),
                        "check-novalidate".as_ref(),
                        "-s".as_ref(),
                        signature_file_path,
                    ],
                );

                match output {
                    Ok(_) => Ok(Verification::new(SigStatus::Unknown, None, None)),
                    Err(_) => Ok(Verification::new(SigStatus::Bad, None, None)),
                }
            }
            Some(principal) => {
                let allowed_signers = self.get_allowed_signers()?;

                let output = self.run(
                    data,
                    &[
                        "-Y".as_ref(),
                        "verify".as_ref(),
                        "-s".as_ref(),
                        signature_file_path,
                        "-I".as_ref(),
                        principal.as_ref(),
                        "-f".as_ref(),
                        allowed_signers.as_ref(),
                    ],
                );

                match output {
                    Ok(_) => Ok(Verification::new(SigStatus::Good, None, Some(principal))),
                    Err(_) => Ok(Verification::new(SigStatus::Bad, None, Some(principal))),
                }
            }
        }
    }
}
