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

use std::ffi::OsString;
use std::fmt::Debug;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use either::Either;
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
    #[error("Failed to parse ssh program response")]
    BadResult,
    #[error("Failed to run ssh-keygen")]
    Io(#[from] std::io::Error),
    #[error("Signing key required")]
    MissingKey,
}

impl From<SshError> for SignError {
    fn from(e: SshError) -> Self {
        SignError::Backend(Box::new(e))
    }
}

type SshResult<T> = Result<T, SshError>;

fn parse_utf8_string(data: Vec<u8>) -> SshResult<String> {
    String::from_utf8(data).map_err(|_| SshError::BadResult)
}

fn run_command(command: &mut Command, stdin: &[u8]) -> SshResult<Vec<u8>> {
    tracing::info!(?command, "running SSH signing command");
    let process = command.spawn()?;
    let write_result = process.stdin.as_ref().unwrap().write_all(stdin);
    let output = process.wait_with_output()?;
    tracing::info!(?command, ?output.status, "SSH signing command exited");
    if output.status.success() {
        write_result?;
        Ok(output.stdout)
    } else {
        Err(SshError::Command {
            exit_status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim_end().into(),
        })
    }
}

// This attempts to convert given key data into a file and return the filepath.
// If the given data is actually already a filepath to a key on disk then the
// key input is returned directly.
fn ensure_key_as_file(key: &str) -> SshResult<Either<PathBuf, tempfile::TempPath>> {
    let is_inlined_ssh_key = key.starts_with("ssh-");
    if !is_inlined_ssh_key {
        let key_path = Path::new(key);
        return Ok(either::Left(key_path.to_path_buf()));
    }

    let mut pub_key_file = tempfile::Builder::new()
        .prefix("jj-signing-key-")
        .tempfile()
        .map_err(SshError::Io)?;

    pub_key_file
        .write_all(key.as_bytes())
        .map_err(SshError::Io)?;
    pub_key_file.flush().map_err(SshError::Io)?;

    // This is converted into a TempPath so that the underlying file handle is
    // closed. On Windows systems this is required for other programs to be able
    // to open the file for reading.
    let pub_key_path = pub_key_file.into_temp_path();
    Ok(either::Right(pub_key_path))
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
                .map_or(None, |v| Some(v.into())),
        )
    }

    fn create_command(&self) -> Command {
        let mut command = Command::new(&self.program);

        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        command
    }

    fn find_principal(&self, signature_file_path: &Path) -> Result<Option<String>, SshError> {
        let Some(allowed_signers) = &self.allowed_signers else {
            return Ok(None);
        };

        let mut command = self.create_command();

        command
            .arg("-Y")
            .arg("find-principals")
            .arg("-f")
            .arg(allowed_signers)
            .arg("-s")
            .arg(signature_file_path);

        // We can't use the existing run_command helper here as `-Y find-principals`
        // will return a non-0 exit code if no principals are found.
        //
        // In this case we don't want to error out, just return None.
        tracing::info!(?command, "running SSH signing command");
        let process = command.spawn()?;
        let output = process.wait_with_output()?;
        tracing::info!(?command, ?output.status, "SSH signing command exited");

        let principal = parse_utf8_string(output.stdout)?
            .split('\n')
            .next()
            .unwrap()
            .trim()
            .to_string();

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
        signature.starts_with(b"-----BEGIN SSH SIGNATURE-----")
    }

    fn sign(&self, data: &[u8], key: Option<&str>) -> Result<Vec<u8>, SignError> {
        let Some(key) = key else {
            return Err(SshError::MissingKey.into());
        };

        // The ssh-keygen `-f` flag expects to be given a file which contains either a
        // private or public key.
        //
        // As it expects a file and we might have an inlined public key instead, we need
        // to ensure it is written to a file first.
        let pub_key_path = ensure_key_as_file(key)?;
        let mut command = self.create_command();

        let path = match &pub_key_path {
            either::Left(path) => path.as_os_str(),
            either::Right(path) => path.as_os_str(),
        };

        command
            .arg("-Y")
            .arg("sign")
            .arg("-f")
            .arg(path)
            .arg("-n")
            .arg("git");

        Ok(run_command(&mut command, data)?)
    }

    fn verify(&self, data: &[u8], signature: &[u8]) -> Result<Verification, SignError> {
        let mut signature_file = tempfile::Builder::new()
            .prefix(".jj-ssh-sig-")
            .tempfile()
            .map_err(SshError::Io)?;
        signature_file.write_all(signature).map_err(SshError::Io)?;
        signature_file.flush().map_err(SshError::Io)?;

        let signature_file_path = signature_file.into_temp_path();

        let principal = self.find_principal(&signature_file_path)?;

        let mut command = self.create_command();

        match (principal, self.allowed_signers.as_ref()) {
            (Some(principal), Some(allowed_signers)) => {
                command
                    .arg("-Y")
                    .arg("verify")
                    .arg("-s")
                    .arg(&signature_file_path)
                    .arg("-I")
                    .arg(&principal)
                    .arg("-f")
                    .arg(allowed_signers)
                    .arg("-n")
                    .arg("git");

                let result = run_command(&mut command, data);

                let status = match result {
                    Ok(_) => SigStatus::Good,
                    Err(_) => SigStatus::Bad,
                };
                Ok(Verification::new(status, None, Some(principal)))
            }
            _ => {
                command
                    .arg("-Y")
                    .arg("check-novalidate")
                    .arg("-s")
                    .arg(&signature_file_path)
                    .arg("-n")
                    .arg("git");

                let result = run_command(&mut command, data);

                match result {
                    Ok(_) => Ok(Verification::new(
                        SigStatus::Unknown,
                        None,
                        Some("Signature OK. Unknown principal".into()),
                    )),
                    Err(_) => Ok(Verification::new(SigStatus::Bad, None, None)),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::Read;

    use super::*;

    #[test]
    fn test_ssh_key_to_file_conversion_raw_key_data() {
        let keydata = "ssh-ed25519 some-key-data";
        let path = ensure_key_as_file(keydata).unwrap();

        let mut buf = vec![];
        let mut file = File::open(path.right().unwrap()).unwrap();
        file.read_to_end(&mut buf).unwrap();

        assert_eq!("ssh-ed25519 some-key-data", String::from_utf8(buf).unwrap());
    }

    #[test]
    fn test_ssh_key_to_file_conversion_existing_file() {
        let mut file = tempfile::Builder::new()
            .prefix("jj-signing-key-")
            .tempfile()
            .map_err(SshError::Io)
            .unwrap();

        file.write_all(b"some-data").map_err(SshError::Io).unwrap();
        file.flush().map_err(SshError::Io).unwrap();

        let file_path = file.into_temp_path();

        let path = ensure_key_as_file(file_path.to_str().unwrap()).unwrap();

        assert_eq!(
            file_path.to_str().unwrap(),
            path.left().unwrap().to_str().unwrap()
        );
    }
}
