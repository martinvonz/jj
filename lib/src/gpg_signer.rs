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

use std::ffi::{OsStr, OsString};
use std::fmt::Debug;
use std::io::Write;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::RwLock;

use thiserror::Error;

use crate::signing::{SignError, SigningBackend, Verification, VerificationStatus};

#[derive(Debug)]
pub struct GpgSigner {
    key: RwLock<Key>,
    program: OsString,
}

#[derive(Debug, Error)]
pub enum GpgError {
    #[error("GPG failed with exit status {exit_status}:\n{stderr}")]
    Command {
        exit_status: ExitStatus,
        stderr: String,
    },
    #[error("Failed to parse GPG output: {0}")]
    ParseFail(&'static str),
    #[error("Failed to run GPG: {0}")]
    Io(#[from] std::io::Error),
}

impl From<GpgError> for SignError {
    fn from(e: GpgError) -> Self {
        SignError::Other(Box::new(e))
    }
}

// since it's hex-encoded we could've condensed this down to a u64
// but then such condensation would also have to happen every time we read GPG
// output
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
struct KeyId([u8; 16]);

impl TryFrom<&[u8]> for KeyId {
    type Error = GpgError;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        value
            .try_into()
            .map(Self)
            .map_err(|_| GpgError::ParseFail("Invalid key id length"))
    }
}

impl AsRef<str> for KeyId {
    fn as_ref(&self) -> &str {
        std::str::from_utf8(&self.0).unwrap()
    }
}

impl Debug for KeyId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("KeyId").field(&self.as_ref()).finish()
    }
}

#[derive(Debug, Clone)]
enum Key {
    Named(Option<String>),
    Resolved(KeyId),
}

impl GpgSigner {
    pub fn new(program: OsString) -> Self {
        Self {
            program,
            key: RwLock::new(Key::Named(None)),
        }
    }

    pub fn from_config(config: &config::Config) -> Self {
        Self::new(
            config
                .get_string("sign.backengs.gpg.program")
                .unwrap_or_else(|_| "gpg".into())
                .into(),
        )
    }

    fn _run<const CHECK_RES: bool>(
        &self,
        input: &[u8],
        args: &[&OsStr],
    ) -> Result<Vec<u8>, GpgError> {
        let process = Command::new(&self.program)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(if CHECK_RES {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .args(args)
            .spawn()?;
        process.stdin.as_ref().unwrap().write_all(input)?;
        let output = process.wait_with_output()?;
        if CHECK_RES && !output.status.success() {
            Err(GpgError::Command {
                exit_status: output.status,
                stderr: String::from_utf8_lossy(&output.stderr).into(),
            })
        } else {
            Ok(output.stdout)
        }
    }

    fn run(&self, input: &[u8], args: &[&OsStr]) -> Result<Vec<u8>, GpgError> {
        self._run::<true>(input, args)
    }

    fn run_ignoring(&self, input: &[u8], args: &[&OsStr]) -> Result<Vec<u8>, GpgError> {
        self._run::<false>(input, args)
    }

    fn get_key_id(&self) -> Result<KeyId, GpgError> {
        let user_id = match &*self.key.read().unwrap() {
            Key::Resolved(key_id) => return Ok(*key_id),
            Key::Named(user_id) => user_id.clone(),
        };
        let mut args = vec!["--with-colons".as_ref(), "--list-secret-keys".as_ref()];
        if let Some(user_id) = &user_id {
            args.push(user_id.as_ref());
        }
        let output = self.run(b"", &args)?;

        // gpg uses the first matched key with it's -u option, and so do we
        let fpr = output
            .split(|b| *b == b'\n')
            .find(|line| line.starts_with(b"fpr:")) // get *the first* fpr line
            .ok_or(GpgError::ParseFail("No key found found for given user id"))?
            .split(|b| *b == b':')
            .nth(9)
            .ok_or(GpgError::ParseFail("Invalid fpr line"))?;
        if fpr.len() != 40 {
            return Err(GpgError::ParseFail("Invalid fingerprint length"));
        }
        let key_id = fpr[24..40].try_into().unwrap();
        *self.key.write().unwrap() = Key::Resolved(key_id);

        Ok(key_id)
    }
}

impl SigningBackend for GpgSigner {
    fn update_key(&self, key: Option<String>) {
        *self.key.write().unwrap() = Key::Named(key);
    }

    fn is_of_this_type(&self, signature: &[u8]) -> bool {
        signature.starts_with(b"-----BEGIN PGP SIGNATURE-----")
    }

    fn is_own(&self, signature: &[u8]) -> Result<bool, SignError> {
        // output of verify also contains both the fingerprint and even the full user id
        // but calling it with some dummy data feels more hacky, and this works anyway
        let output = self.run(
            signature,
            &["--list-packets".as_ref(), "--keyid-format=long".as_ref()],
        )?;

        let key_id = output
            .split(|b| *b == b'\n')
            .find(|line| line.starts_with(b":signature packet:"))
            .ok_or(GpgError::ParseFail("No signature packets found"))?
            .rsplitn(2, |b| *b == b' ') // take the last "field"
            .next()
            .ok_or(GpgError::ParseFail("Invalid signature packet line"))?;

        Ok(KeyId::try_from(key_id)? == self.get_key_id()?)
    }

    fn sign(&self, data: &[u8]) -> Result<Vec<u8>, SignError> {
        let key_id = self.get_key_id()?;
        Ok(self.run(data, &["-abu".as_ref(), key_id.as_ref().as_ref()])?)
    }

    fn verify(&self, data: &[u8], signature: &[u8]) -> Result<Verification, SignError> {
        let mut signature_file = tempfile::Builder::new()
            .prefix(".jj-gpg-sig-tmp-")
            .tempfile()
            .map_err(GpgError::Io)?;
        signature_file.write_all(signature).map_err(GpgError::Io)?;
        signature_file.flush().map_err(GpgError::Io)?;

        let output = self.run_ignoring(
            data,
            &[
                "--status-fd=1".as_ref(),
                "--verify".as_ref(),
                signature_file.path().as_os_str(), /* the only reason we have those .as_refs
                                                    * transmuting to &OsStr everywhere.. */
                "-".as_ref(),
            ],
        )?;

        // find the line "<prefix> [key] [display with spaces]" and return them as
        // strings
        fn find_stuff(output: &[u8], prefix: &[u8]) -> Option<(Option<String>, Option<String>)> {
            output
                .split(|b| *b == b'\n')
                .find(|line| line.starts_with(prefix))
                .map(|line| {
                    let mut iter = line[prefix.len()..].splitn(2, |b| *b == b' ');
                    let a = iter
                        .next()
                        .and_then(|bs| String::from_utf8(bs.to_owned()).ok());
                    let b = iter
                        .next()
                        .and_then(|bs| String::from_utf8(bs.to_owned()).ok());
                    (a, b)
                })
        }

        if let Some((key, display)) = find_stuff(&output, b"[GNUPG:] GOODSIG ") {
            Ok(Verification {
                status: VerificationStatus::Good,
                key,
                display,
            })
        } else if let Some((key, display)) = find_stuff(&output, b"[GNUPG:] NO_PUBKEY ") {
            Ok(Verification {
                status: VerificationStatus::Unknown,
                key,
                display, // display is always None here
            })
        } else if let Some((key, display)) = find_stuff(&output, b"[GNUPG:] BADSIG ") {
            Ok(Verification {
                status: VerificationStatus::Bad,
                key,
                display,
            })
        } else {
            Ok(Verification::invalid())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires having gpg in path, just a temp manual test with printouts"]
    fn test_gpg_signer() {
        let signer = GpgSigner::new("gpg".into());

        println!("identity before: {:x?}", signer.key.read().unwrap());

        let data = b"Hello, world!";
        let signature = signer.sign(data).unwrap();

        println!("identity after: {:x?}", signer.key.read().unwrap());

        let verify = signer.verify(data, &signature).unwrap();
        let verify2 = signer
            .verify(
                b"",
                b"-----BEGIN PGP SIGNATURE-----

            iQIzBAEBCAAdFiEEalG7q7o9VGe2FxIhgJqNfOsQtGQFAmQ5B+cACgkQgJqNfOsQ
            tGSmOBAAnQVdrztVVw+SPkJnY2bM8icmCZvEQxsN7zglhc6IubA710HtbO03vrsr
            1p7V/DoSOOsegXd9KIH618Li/6O2zx6tELzhZVaHOKvT/xlM7jh/ZqcUVhop65Jy
            sXiCIdKabfyxkHoq0GBzsGGmU3n3GUlQmsNfvUXoghawUNKOE1+VgV4RLGEuUNrT
            5IViT4Ct6Ojq+Sk9Gj9b7ghepRzQZ0ZpZJ19ms8pK2CEPHSEnKWOMGFp7Ho0iEzG
            9u+DLY20De1GV8cdxQ+vCGcc8KL3wFHkZvZkU5TrlHODUa/+NvihdCzLtNuRM4u4
            ckJo9WitN4FpySlv0WKR2jC3WTi1Zsw/lvR2uXv4DSsa9hdu2DpUOYCvCCIMtoXw
            j5lE4/2fNLlahsgD8NACtI3ulomM/VkhIHtGR7dT43jTCsrmkPSbTeGcwUSgGTjM
            vZ24gJHHb3y84jF6o3VbfNfHRAVZx3H02MQOlRlresleOgVXwWWwYroxGtdYSsQK
            p/bCOoaOlmFcrG7rLqR0SG1IqBhdW5egT/U/Et+7xPztbxR3SmRd8CShLYD2VTFp
            UAtn9w/qGAo/BSuS+5XPpAiX9KxOhhK01bB+Hc26tzAeEYp4O6382DVdTDmuvv/v
            IbQa9yao7yboowsKbe3Cv6axMlVqNcZsulawmQ2r8YVEzBPUrgQ=
            =7dH0
            -----END PGP SIGNATURE-----",
            )
            .unwrap();
        let verify3 = signer
            .verify(
                // b"\n",
                b"",
                b"-----BEGIN PGP SIGNATURE-----

                iQJGBAABCAAwFiEEaW8UKZw5HuvbGCSh1mZ5mv/YUCoFAmSZYZISHHNlbGZAbmVj
                YXVxdWEuZGV2AAoJENZmeZr/2FAqFQQP/3tSY0Xiq8XCG/vLeT4M+nGEh+Hjg8yn
                dUgAM+7D4l0BsHoRfH8I/TWbV7cVCXBNrEaeT8db8sSkvjqr3YX3050cYGxbIkMO
                /QIifcb8TINZtVYM2gJ3wlTiEyjn6O70YG7yFsR3/61Ih6DRL2dIZyfTWkdKj/sm
                EOUcmsKxV4sT2q4mK38BmNCfT00PtbooFilN9YEebMFWrfSwzkrixTraExnPnyW0
                b7YdmZJaVJdBBLodqzt13BkznzY7rWkX7S9d7N4RHOo+MKAqs5M4o7TgYk8JtOVY
                3AbAHyvCd+guG2Up458knk3Q2xQ3SfLD7obk0WZ+dF/ZstFDiSi6Qp+U54dzwim1
                Kq1okzW4hlTOdzWpY+1/AmdIx5+RbAYK9p2nBotuG/UkqVrjtt3Hy4VUV+/PZ7Sc
                tzpW/BpVD2s8TbezBxo/gcwuVSJNnRWk0j4Q0C4TxxntaxrDIJ0fw8NjzvVdnijB
                YpD/0o9nmwKtL05WRu+EVj1Q37VrMxXarjeVTzjTpiqyQM/E97TQinAFwivIEmv6
                JM3zXnI4xIX/BsEtQMjKo546gbcVRCjABvkc820udTFPPRwXXznFWZqOQaomhCZ8
                ZwjdGUdKQmAv0iyoVAHwgnDvQ5f5ziGXODj7qtEkpSnHBGO8/tlyQ+OasAYH1KuX
                9PGm4YHqrSzn
                =L53R
                -----END PGP SIGNATURE-----",
            )
            .unwrap();
        let verify4 = signer.verify(b"", b"hello there").unwrap();

        // Good (was just signed, reflexive)
        dbg!(verify);
        // Unknown (some fedora download signature, we don't have the public key)
        dbg!(verify2);
        // Bad (a \n signed by my git signing key, but we check empty bytestring)
        dbg!(verify3);
        // Invalid (well, it is)
        dbg!(verify4);
    }
}
