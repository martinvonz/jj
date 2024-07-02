use hex::ToHex;
use jj_lib::content_hash::blake2b_hash;
use jj_lib::signing::{SigStatus, SignError, SignResult, SigningBackend, Verification};

#[derive(Debug)]
pub struct TestSigningBackend;

const PREFIX: &str = "--- JJ-TEST-SIGNATURE ---\nKEY: ";

impl SigningBackend for TestSigningBackend {
    fn name(&self) -> &str {
        "test"
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
        if sig == signature {
            Ok(Verification {
                status: SigStatus::Good,
                key,
                display: None,
            })
        } else {
            Ok(Verification {
                status: SigStatus::Bad,
                key,
                display: None,
            })
        }
    }
}
