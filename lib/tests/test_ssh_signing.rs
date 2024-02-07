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

use std::fs;
#[cfg(unix)]
use std::fs::Permissions;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::prelude::PermissionsExt;
use std::path::{Path, PathBuf};

use jj_lib::signing::{SigStatus, SigningBackend};
use jj_lib::ssh_signing::SshBackend;

static PRIVATE_KEY: &str = r#"-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACBo/iejekjvuD/HTman0daImstssYYR52oB+dmr1KsOYQAAAIiuGFMFrhhT
BQAAAAtzc2gtZWQyNTUxOQAAACBo/iejekjvuD/HTman0daImstssYYR52oB+dmr1KsOYQ
AAAECcUtn/J/jk/+D5+/+WbQRNN4eInj5L60pt6FioP0nQfGj+J6N6SO+4P8dOZqfR1oia
y2yxhhHnagH52avUqw5hAAAAAAECAwQF
-----END OPENSSH PRIVATE KEY-----
"#;

static PUBLIC_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGj+J6N6SO+4P8dOZqfR1oiay2yxhhHnagH52avUqw5h";

struct SshEnvironment {
    _keys: tempfile::TempDir,
    private_key_path: PathBuf,
    allowed_signers: Option<tempfile::TempPath>,
}

impl SshEnvironment {
    fn new() -> Result<Self, std::process::Output> {
        let keys_dir = tempfile::Builder::new()
            .prefix("jj-test-signing-keys-")
            .tempdir()
            .unwrap();

        let private_key_path = Path::new(keys_dir.path()).join("key");

        fs::write(&private_key_path, PRIVATE_KEY).unwrap();

        #[cfg(unix)]
        std::fs::set_permissions(&private_key_path, Permissions::from_mode(0o700)).unwrap();

        let mut env = SshEnvironment {
            _keys: keys_dir,
            private_key_path,
            allowed_signers: None,
        };

        env.with_good_public_key();

        Ok(env)
    }

    fn with_good_public_key(&mut self) {
        let mut allowed_signers = tempfile::Builder::new()
            .prefix("jj-test-allowed-signers-")
            .tempfile()
            .unwrap();

        allowed_signers
            .write_all("test@example.com ".as_bytes())
            .unwrap();
        allowed_signers.write_all(PUBLIC_KEY.as_bytes()).unwrap();
        allowed_signers.flush().unwrap();

        let allowed_signers_path = allowed_signers.into_temp_path();

        self.allowed_signers = Some(allowed_signers_path);
    }

    fn with_bad_public_key(&mut self) {
        let mut allowed_signers = tempfile::Builder::new()
            .prefix("jj-test-allowed-signers-")
            .tempfile()
            .unwrap();

        allowed_signers
            .write_all("test@example.com ".as_bytes())
            .unwrap();
        allowed_signers
            .write_all("INVALID PUBLIC KEY".as_bytes())
            .unwrap();
        allowed_signers.flush().unwrap();

        let allowed_signers_path = allowed_signers.into_temp_path();

        self.allowed_signers = Some(allowed_signers_path);
    }
}

fn backend(env: &SshEnvironment) -> SshBackend {
    SshBackend::new(
        "ssh-keygen".into(),
        env.allowed_signers
            .as_ref()
            .map(|allowed_signers| allowed_signers.as_os_str().into()),
    )
}

#[test]
fn ssh_signing_roundtrip() {
    let env = SshEnvironment::new().unwrap();
    let backend = backend(&env);
    let data = b"hello world";

    let signature = backend
        .sign(data, Some(env.private_key_path.to_str().unwrap()))
        .unwrap();

    let check = backend.verify(data, &signature).unwrap();
    assert_eq!(check.status, SigStatus::Good);

    assert_eq!(check.display.unwrap(), "test@example.com");

    let check = backend.verify(b"invalid-commit-data", &signature).unwrap();
    assert_eq!(check.status, SigStatus::Bad);
    assert_eq!(check.display.unwrap(), "test@example.com");
}

#[test]
fn ssh_signing_bad_allowed_signers() {
    let mut env = SshEnvironment::new().unwrap();
    env.with_bad_public_key();

    let backend = backend(&env);
    let data = b"hello world";

    let signature = backend
        .sign(data, Some(env.private_key_path.to_str().unwrap()))
        .unwrap();

    let check = backend.verify(data, &signature).unwrap();
    assert_eq!(check.status, SigStatus::Unknown);
    assert_eq!(check.display.unwrap(), "Signature OK. Unknown principal");
}

#[test]
fn ssh_signing_missing_allowed_signers() {
    let mut env = SshEnvironment::new().unwrap();
    env.allowed_signers = None;

    let backend = backend(&env);
    let data = b"hello world";

    let signature = backend
        .sign(data, Some(env.private_key_path.to_str().unwrap()))
        .unwrap();

    let check = backend.verify(data, &signature).unwrap();
    assert_eq!(check.status, SigStatus::Unknown);
    assert_eq!(check.display.unwrap(), "Signature OK. Unknown principal");
}
