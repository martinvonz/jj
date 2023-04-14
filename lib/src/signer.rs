#[cfg(feature = "signing-gpgme")]
use gpgme::{Context, Key, Protocol};

use crate::backend::BackendResult;

pub trait Signer {
    fn sign(&self, input: &[u8]) -> BackendResult<String>;
}

#[cfg(feature = "signing-gpgme")]
pub struct GpgMeSigner {
    key: Option<Key>,
}

#[cfg(feature = "signing-gpgme")]
impl GpgMeSigner {
    pub fn new(key_id: Option<String>) -> Self {
        let key = match key_id {
            Some(key_id) => {
                let mut ctx =
                    Context::from_protocol(Protocol::OpenPgp).expect("from_protocol failed");
                let mut keys = ctx.find_keys([key_id]).expect("failed to find keys");
                let key = keys
                    .next()
                    .expect("no key found")
                    .expect("failed to get key");
                Some(key)
            }
            None => None,
        };
        Self { key }
    }
}

#[cfg(feature = "signing-gpgme")]
impl Signer for GpgMeSigner {
    fn sign(&self, input: &[u8]) -> BackendResult<String> {
        let mut ctx = Context::from_protocol(Protocol::OpenPgp).unwrap();
        let mut outbuf = Vec::new();

        if let Some(key) = &self.key {
            ctx.add_signer(key).unwrap();
        }

        ctx.set_armor(true);
        ctx.sign_detached(input, &mut outbuf).unwrap();

        let out = String::from_utf8(outbuf).unwrap();
        Ok(out)
    }
}
