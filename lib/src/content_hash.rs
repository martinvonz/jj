//! Portable, stable hashing suitable for identifying values

use blake2::Blake2b512;
// Re-export DigestUpdate so that the ContentHash proc macro can be used in
// external crates without directly depending on the digest crate.
pub use digest::Update as DigestUpdate;
use itertools::Itertools as _;
pub use jj_lib_proc_macros::ContentHash;

/// Portable, stable hashing suitable for identifying values
///
/// Variable-length sequences should hash a 64-bit little-endian representation
/// of their length, then their elements in order. Unordered containers should
/// order their elements according to their `Ord` implementation. Enums should
/// hash a 32-bit little-endian encoding of the ordinal number of the enum
/// variant, then the variant's fields in lexical order.
///
/// Structs can implement `ContentHash` by using `#[derive(ContentHash)]`.
pub trait ContentHash {
    /// Update the hasher state with this object's content
    fn hash(&self, state: &mut impl DigestUpdate);
}

/// The 512-bit BLAKE2b content hash
pub fn blake2b_hash(x: &(impl ContentHash + ?Sized)) -> digest::Output<Blake2b512> {
    use digest::Digest;
    let mut hasher = Blake2b512::default();
    x.hash(&mut hasher);
    hasher.finalize()
}

impl ContentHash for () {
    fn hash(&self, _: &mut impl DigestUpdate) {}
}

impl ContentHash for bool {
    fn hash(&self, state: &mut impl DigestUpdate) {
        u8::from(*self).hash(state);
    }
}

impl ContentHash for u8 {
    fn hash(&self, state: &mut impl DigestUpdate) {
        state.update(&[*self]);
    }
}

impl ContentHash for u32 {
    fn hash(&self, state: &mut impl DigestUpdate) {
        state.update(&self.to_le_bytes());
    }
}

impl ContentHash for i32 {
    fn hash(&self, state: &mut impl DigestUpdate) {
        state.update(&self.to_le_bytes());
    }
}

impl ContentHash for u64 {
    fn hash(&self, state: &mut impl DigestUpdate) {
        state.update(&self.to_le_bytes());
    }
}

impl ContentHash for i64 {
    fn hash(&self, state: &mut impl DigestUpdate) {
        state.update(&self.to_le_bytes());
    }
}

// TODO: Specialize for [u8] once specialization exists
impl<T: ContentHash> ContentHash for [T] {
    fn hash(&self, state: &mut impl DigestUpdate) {
        state.update(&(self.len() as u64).to_le_bytes());
        for x in self {
            x.hash(state);
        }
    }
}

impl<T: ContentHash> ContentHash for Vec<T> {
    fn hash(&self, state: &mut impl DigestUpdate) {
        self.as_slice().hash(state)
    }
}

impl ContentHash for String {
    fn hash(&self, state: &mut impl DigestUpdate) {
        self.as_bytes().hash(state);
    }
}

impl<T: ContentHash> ContentHash for Option<T> {
    fn hash(&self, state: &mut impl DigestUpdate) {
        match self {
            None => state.update(&0u32.to_le_bytes()),
            Some(x) => {
                state.update(&1u32.to_le_bytes());
                x.hash(state)
            }
        }
    }
}

impl<K, V> ContentHash for std::collections::HashMap<K, V>
where
    K: ContentHash + Ord,
    V: ContentHash,
{
    fn hash(&self, state: &mut impl DigestUpdate) {
        state.update(&(self.len() as u64).to_le_bytes());
        let mut kv = self.iter().collect_vec();
        kv.sort_unstable_by_key(|&(k, _)| k);
        for (k, v) in kv {
            k.hash(state);
            v.hash(state);
        }
    }
}

impl<K> ContentHash for std::collections::HashSet<K>
where
    K: ContentHash + Ord,
{
    fn hash(&self, state: &mut impl DigestUpdate) {
        state.update(&(self.len() as u64).to_le_bytes());
        for k in self.iter().sorted() {
            k.hash(state);
        }
    }
}

impl<K, V> ContentHash for std::collections::BTreeMap<K, V>
where
    K: ContentHash,
    V: ContentHash,
{
    fn hash(&self, state: &mut impl DigestUpdate) {
        state.update(&(self.len() as u64).to_le_bytes());
        for (k, v) in self.iter() {
            k.hash(state);
            v.hash(state);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use super::*;

    #[test]
    fn test_string_sanity() {
        let a = "a".to_string();
        let b = "b".to_string();
        assert_eq!(hash(&a), hash(&a.clone()));
        assert_ne!(hash(&a), hash(&b));
        assert_ne!(hash(&"a".to_string()), hash(&"a\0".to_string()));
    }

    #[test]
    fn test_hash_map_key_value_distinction() {
        let a = [("ab".to_string(), "cd".to_string())]
            .into_iter()
            .collect::<HashMap<_, _>>();
        let b = [("a".to_string(), "bcd".to_string())]
            .into_iter()
            .collect::<HashMap<_, _>>();

        assert_ne!(hash(&a), hash(&b));
    }

    #[test]
    fn test_btree_map_key_value_distinction() {
        let a = [("ab".to_string(), "cd".to_string())]
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let b = [("a".to_string(), "bcd".to_string())]
            .into_iter()
            .collect::<BTreeMap<_, _>>();

        assert_ne!(hash(&a), hash(&b));
    }

    #[test]
    fn test_struct_sanity() {
        #[derive(ContentHash)]
        struct Foo {
            x: i32,
        }
        assert_ne!(hash(&Foo { x: 42 }), hash(&Foo { x: 12 }));
    }

    #[test]
    fn test_option_sanity() {
        assert_ne!(hash(&Some(42)), hash(&42));
        assert_ne!(hash(&None::<i32>), hash(&42i32));
    }

    #[test]
    fn test_slice_sanity() {
        assert_ne!(hash(&[42i32][..]), hash(&[12i32][..]));
        assert_ne!(hash(&([] as [i32; 0])[..]), hash(&[42i32][..]));
        assert_ne!(hash(&([] as [i32; 0])[..]), hash(&()));
        assert_ne!(hash(&42i32), hash(&[42i32][..]));
    }

    #[test]
    fn test_consistent_hashing() {
        #[derive(ContentHash)]
        struct Foo {
            x: Vec<Option<i32>>,
            y: i64,
        }
        let foo_hash = hex::encode(hash(&Foo {
            x: vec![None, Some(42)],
            y: 17,
        }));
        insta::assert_snapshot!(
            foo_hash,
            @"e33c423b4b774b1353c414e0f9ef108822fde2fd5113fcd53bf7bd9e74e3206690b96af96373f268ed95dd020c7cbe171c7b7a6947fcaf5703ff6c8e208cefd4"
        );

        // Try again with an equivalent generic struct deriving ContentHash.
        #[derive(ContentHash)]
        struct GenericFoo<X, Y> {
            x: X,
            y: Y,
        }
        assert_eq!(
            hex::encode(hash(&GenericFoo {
                x: vec![None, Some(42)],
                y: 17i64
            })),
            foo_hash
        );
    }

    // Test that the derived version of `ContentHash` matches the that's
    // manually implemented for `std::Option`.
    #[test]
    fn derive_for_enum() {
        #[derive(ContentHash)]
        enum MyOption<T> {
            None,
            Some(T),
        }
        assert_eq!(hash(&Option::<i32>::None), hash(&MyOption::<i32>::None));
        assert_eq!(hash(&Some(1)), hash(&MyOption::Some(1)));
    }

    fn hash(x: &(impl ContentHash + ?Sized)) -> digest::Output<Blake2b512> {
        blake2b_hash(x)
    }
}
