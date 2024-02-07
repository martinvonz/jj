use std::io::Write;
use std::process::Stdio;

use jj_lib::signing::{SigStatus, SigningBackend};
use jj_lib::ssh_signing::SshBackend;
use rustix::path::Arg;

static PRIVATE_KEY: &'static str = r#"-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACBo/iejekjvuD/HTman0daImstssYYR52oB+dmr1KsOYQAAAIiuGFMFrhhT
BQAAAAtzc2gtZWQyNTUxOQAAACBo/iejekjvuD/HTman0daImstssYYR52oB+dmr1KsOYQ
AAAECcUtn/J/jk/+D5+/+WbQRNN4eInj5L60pt6FioP0nQfGj+J6N6SO+4P8dOZqfR1oia
y2yxhhHnagH52avUqw5hAAAAAAECAwQF
-----END OPENSSH PRIVATE KEY-----
"#;

static PUBLIC_KEY: &'static str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGj+J6N6SO+4P8dOZqfR1oiay2yxhhHnagH52avUqw5h";

static PUBLIC_KEY_BAD: &'static str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGj+J6N6SO";

fn prepare() {
    let mut pk = tempfile::Builder::new()
        .prefix("jj-test-pk-")
        .tempfile()
        .unwrap();

    pk.write_all(PRIVATE_KEY.as_bytes()).unwrap();
    pk.flush().unwrap();

    let mut ssh_add = std::process::Command::new("ssh-add")
        .arg(pk.path().as_str().unwrap())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    ssh_add.wait().unwrap();
}

fn cleanup() {
    let mut ssh_add = std::process::Command::new("ssh-add")
        .arg("-d")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    ssh_add
        .stdin
        .as_mut()
        .unwrap()
        .write_all(PUBLIC_KEY.as_bytes())
        .unwrap();

    ssh_add.stdin.as_mut().unwrap().flush().unwrap();

    ssh_add.wait().unwrap();
}

fn backend(public_key: &str) -> (SshBackend, tempfile::NamedTempFile) {
    let mut allowed_signers = tempfile::Builder::new()
        .prefix("jj-test-allowed-signers-")
        .tempfile()
        .unwrap();

    allowed_signers
        .write_all("test@example.com ".as_bytes())
        .unwrap();
    allowed_signers.write_all(public_key.as_bytes()).unwrap();
    allowed_signers.flush().unwrap();

    let backend = SshBackend::new(
        "ssh-keygen".into(),
        Some(allowed_signers.path().as_str().unwrap().into()),
    );

    (backend, allowed_signers)
}

#[test]
fn roundtrip() {
    let (backend, _allowed_signers) = backend(PUBLIC_KEY);
    let data = b"hello world";

    prepare();
    let signature = backend.sign(data, Some(PUBLIC_KEY)).unwrap();

    let check = backend.verify(data, &signature).unwrap();
    assert_eq!(check.status, SigStatus::Good);
    assert_eq!(check.backend(), None); // backend is set by the signer

    assert_eq!(check.display.unwrap(), "test@example.com");

    let check = backend.verify(b"invalid-commit-data", &signature).unwrap();
    assert_eq!(check.status, SigStatus::Bad);
    assert_eq!(check.backend(), None);
    assert_eq!(check.display.unwrap(), "test@example.com");

    cleanup();
}

#[test]
fn bad_allowed_signers() {
    let (backend, _allowed_signers) = backend(PUBLIC_KEY_BAD);
    let data = b"hello world";

    prepare();

    let signature = backend.sign(data, Some(PUBLIC_KEY)).unwrap();

    let check = backend.verify(data, &signature).unwrap();
    assert_eq!(check.status, SigStatus::Unknown);
    assert_eq!(check.display.unwrap(), "Signature OK. Unknown principal");

    cleanup();
}
