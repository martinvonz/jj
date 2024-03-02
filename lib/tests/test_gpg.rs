#[cfg(unix)]
use std::fs::Permissions;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::prelude::PermissionsExt;
use std::process::{Command, Stdio};

use assert_matches::assert_matches;
use insta::assert_debug_snapshot;
use jj_lib::gpg_signing::GpgBackend;
use jj_lib::signing::{SigStatus, SignError, SigningBackend};

static PRIVATE_KEY: &str = r#"-----BEGIN PGP PRIVATE KEY BLOCK-----

lFgEZWI3pBYJKwYBBAHaRw8BAQdAaPLTNADvDWapjAPlxaUnx3HXQNIlwSz4EZrW
3Z7hxSwAAP9liwHZWJCGI2xW+XNqMT36qpIvoRcd5YPaKYwvnlkG1w+UtDNTb21l
b25lIChqaiB0ZXN0IHNpZ25pbmcga2V5KSA8c29tZW9uZUBleGFtcGxlLmNvbT6I
kwQTFgoAOxYhBKWOXukGcVPI9eXp6WOHhcsW/qBhBQJlYjekAhsDBQsJCAcCAiIC
BhUKCQgLAgQWAgMBAh4HAheAAAoJEGOHhcsW/qBhyBgBAMph1HkBkKlrZmsun+3i
kTEaOsWmaW/D6NEdMFiw0S/jAP9G3jOYGiZbUN3dWWB2246Oi7SaMTX8Xb2BrLP2
axCbC5RYBGVjxv8WCSsGAQQB2kcPAQEHQE8Oa4ahtVG29gIRssPxjqF4utn8iHPz
m5z/8lX/nl3eAAD5AZ6H2pNhiy2gnGkbPLHw3ZyY4d0NXzCa7qc9EXqOj+sRrLQ9
U29tZW9uZSBFbHNlIChqaiB0ZXN0IHNpZ25pbmcga2V5KSA8c29tZW9uZS1lbHNl
QGV4YW1wbGUuY29tPoiTBBMWCgA7FiEER1BAaEpU3TKUiUvFTtVW6XKeAA8FAmVj
xv8CGwMFCwkIBwICIgIGFQoJCAsCBBYCAwECHgcCF4AACgkQTtVW6XKeAA/6TQEA
2DkPm3LmH8uG6qLirtf62kbG7T+qljIsarQKFw3CGakA/AveCtrL7wVSpINiu1Rz
lBqJFFP2PqzT0CRfh94HSIMM
=6JC8
-----END PGP PRIVATE KEY BLOCK-----
"#;

struct GpgEnvironment {
    homedir: tempfile::TempDir,
}

impl GpgEnvironment {
    fn new() -> Result<Self, std::process::Output> {
        let dir = tempfile::Builder::new()
            .prefix("jj-gpg-signing-test-")
            .tempdir()
            .unwrap();

        let path = dir.path();

        #[cfg(unix)]
        std::fs::set_permissions(path, Permissions::from_mode(0o700)).unwrap();

        let mut gpg = std::process::Command::new("gpg")
            .arg("--homedir")
            .arg(path)
            .arg("--import")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        gpg.stdin
            .as_mut()
            .unwrap()
            .write_all(PRIVATE_KEY.as_bytes())
            .unwrap();

        gpg.stdin.as_mut().unwrap().flush().unwrap();

        let res = gpg.wait_with_output().unwrap();

        if !res.status.success() {
            println!("Failed to add private key to gpg-agent. Make sure it is running!");
            println!("{}", String::from_utf8_lossy(&res.stderr));
            return Err(res);
        }

        Ok(GpgEnvironment { homedir: dir })
    }
}

macro_rules! gpg_guard {
    () => {
        if Command::new("gpg").arg("--version").status().is_err() {
            eprintln!("Skipping test because gpg is not installed on the system");
            return;
        }
    };
}

fn backend(env: &GpgEnvironment) -> GpgBackend {
    // don't really need faked time for current tests,
    // but probably will need it for end-to-end cli tests
    GpgBackend::new("gpg".into(), false).with_extra_args(&[
        "--homedir".into(),
        env.homedir.path().as_os_str().into(),
        "--faked-system-time=1701042000!".into(),
    ])
}

#[test]
#[cfg_attr(windows, ignore = "stuck randomly on Windows CI #3140")] // FIXME
fn gpg_signing_roundtrip() {
    gpg_guard!();

    let env = GpgEnvironment::new().unwrap();
    let backend = backend(&env);
    let data = b"hello world";
    let signature = backend.sign(data, None).unwrap();

    let check = backend.verify(data, &signature).unwrap();
    assert_eq!(check.status, SigStatus::Good);
    assert_eq!(check.key.unwrap(), "638785CB16FEA061");
    assert_eq!(
        check.display.unwrap(),
        "Someone (jj test signing key) <someone@example.com>"
    );

    let check = backend.verify(b"so so bad", &signature).unwrap();
    assert_eq!(check.status, SigStatus::Bad);
    assert_eq!(check.key.unwrap(), "638785CB16FEA061");
    assert_eq!(
        check.display.unwrap(),
        "Someone (jj test signing key) <someone@example.com>"
    );
}

#[test]
#[cfg_attr(windows, ignore = "stuck randomly on Windows CI #3140")] // FIXME
fn gpg_signing_roundtrip_explicit_key() {
    gpg_guard!();

    let env = GpgEnvironment::new().unwrap();
    let backend = backend(&env);
    let data = b"hello world";
    let signature = backend.sign(data, Some("Someone Else")).unwrap();

    assert_debug_snapshot!(backend.verify(data, &signature).unwrap(), @r###"
    Verification {
        status: Good,
        key: Some(
            "4ED556E9729E000F",
        ),
        display: Some(
            "Someone Else (jj test signing key) <someone-else@example.com>",
        ),
    }
    "###);
    assert_debug_snapshot!(backend.verify(b"so so bad", &signature).unwrap(), @r###"
    Verification {
        status: Bad,
        key: Some(
            "4ED556E9729E000F",
        ),
        display: Some(
            "Someone Else (jj test signing key) <someone-else@example.com>",
        ),
    }
    "###);
}

#[test]
#[cfg_attr(windows, ignore = "stuck randomly on Windows CI #3140")] // FIXME
fn unknown_key() {
    gpg_guard!();

    let env = GpgEnvironment::new().unwrap();
    let backend = backend(&env);
    let signature = br"-----BEGIN PGP SIGNATURE-----

    iHUEABYKAB0WIQQs238pU7eC/ROoPJ0HH+PjJN1zMwUCZWPa5AAKCRAHH+PjJN1z
    MyylAP9WQ3sZdbC4b1C+/nxs+Wl+rfwzeQWGbdcsBMyDABcpmgD/U+4KdO7eZj/I
    e+U6bvqw3pOBoI53Th35drQ0qPI+jAE=
    =kwsk
    -----END PGP SIGNATURE-----";
    assert_debug_snapshot!(backend.verify(b"hello world", signature).unwrap(), @r###"
    Verification {
        status: Unknown,
        key: Some(
            "071FE3E324DD7333",
        ),
        display: None,
    }
    "###);
    assert_debug_snapshot!(backend.verify(b"so bad", signature).unwrap(), @r###"
    Verification {
        status: Unknown,
        key: Some(
            "071FE3E324DD7333",
        ),
        display: None,
    }
    "###);
}

#[test]
#[cfg_attr(windows, ignore = "stuck randomly on Windows CI #3140")] // FIXME
fn invalid_signature() {
    gpg_guard!();

    let env = GpgEnvironment::new().unwrap();
    let backend = backend(&env);
    let signature = br"-----BEGIN PGP SIGNATURE-----

    super duper invalid
    -----END PGP SIGNATURE-----";

    // Small data: gpg command will exit late.
    assert_matches!(
        backend.verify(b"a", signature),
        Err(SignError::InvalidSignatureFormat)
    );

    // Large data: gpg command will exit early because the signature is invalid.
    assert_matches!(
        backend.verify(&b"a".repeat(100 * 1024), signature),
        Err(SignError::InvalidSignatureFormat)
    );
}
