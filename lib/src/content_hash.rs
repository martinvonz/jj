//! Portable, stable hashing suitable for identifying values

use blake2::Blake2b512;
use itertools::Itertools as _;

/// Portable, stable hashing suitable for identifying values
///
/// Variable-length sequences should hash a 64-bit little-endian representation
/// of their length, then their elements in order. Unordered containers should
/// order their elements according to their `Ord` implementation. Enums should
/// hash a 32-bit little-endian encoding of the ordinal number of the enum
/// variant, then the variant's fields in lexical order.
pub trait ContentHash {
    /// Update the hasher state with this object's content
    fn hash(&self, state: &mut impl digest::Update);
}

/// The 512-bit BLAKE2b content hash
pub fn blake2b_hash(x: &(impl ContentHash + ?Sized)) -> digest::Output<Blake2b512> {
    use digest::Digest;
    let mut hasher = Blake2b512::default();
    x.hash(&mut hasher);
    hasher.finalize()
}

impl ContentHash for () {
    fn hash(&self, _: &mut impl digest::Update) {}
}

impl ContentHash for bool {
    fn hash(&self, state: &mut impl digest::Update) {
        u8::from(*self).hash(state);
    }
}

impl ContentHash for u8 {
    fn hash(&self, state: &mut impl digest::Update) {
        state.update(&[*self]);
    }
}

impl ContentHash for i32 {
    fn hash(&self, state: &mut impl digest::Update) {
        state.update(&self.to_le_bytes());
    }
}

impl ContentHash for i64 {
    fn hash(&self, state: &mut impl digest::Update) {
        state.update(&self.to_le_bytes());
    }
}

// TODO: Specialize for [u8] once specialization exists
impl<T: ContentHash> ContentHash for [T] {
    fn hash(&self, state: &mut impl digest::Update) {
        state.update(&(self.len() as u64).to_le_bytes());
        for x in self {
            x.hash(state);
        }
    }
}

impl<T: ContentHash> ContentHash for Vec<T> {
    fn hash(&self, state: &mut impl digest::Update) {
        self.as_slice().hash(state)
    }
}

impl ContentHash for String {
    fn hash(&self, state: &mut impl digest::Update) {
        self.as_bytes().hash(state);
    }
}

impl ContentHash for compact_str::CompactString {
    fn hash(&self, state: &mut impl digest::Update) {
        self.as_bytes().hash(state);
    }
}

impl<T: ContentHash> ContentHash for Option<T> {
    fn hash(&self, state: &mut impl digest::Update) {
        match self {
            None => state.update(&[0]),
            Some(x) => {
                state.update(&[1]);
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
    fn hash(&self, state: &mut impl digest::Update) {
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
    fn hash(&self, state: &mut impl digest::Update) {
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
    fn hash(&self, state: &mut impl digest::Update) {
        state.update(&(self.len() as u64).to_le_bytes());
        for (k, v) in self.iter() {
            k.hash(state);
            v.hash(state);
        }
    }
}

macro_rules! content_hash {
    ($(#[$meta:meta])* $vis:vis struct $name:ident {
        $($(#[$field_meta:meta])* $field_vis:vis $field:ident : $ty:ty),* $(,)?
    }) => {
        $(#[$meta])*
        $vis struct $name {
            $($(#[$field_meta])* $field_vis $field : $ty),*
        }

        impl crate::content_hash::ContentHash for $name {
            fn hash(&self, state: &mut impl digest::Update) {
                $(<$ty as crate::content_hash::ContentHash>::hash(&self.$field, state);)*
            }
        }
    };
    ($(#[$meta:meta])* $vis:vis struct $name:ident($field_vis:vis $ty:ty);) => {
        $(#[$meta])*
        $vis struct $name($field_vis $ty);

        impl crate::content_hash::ContentHash for $name {
            fn hash(&self, state: &mut impl digest::Update) {
                <$ty as crate::content_hash::ContentHash>::hash(&self.0, state);
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use blake2::Blake2b512;

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
        content_hash! {
            struct Foo { x: i32 }
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
        content_hash! {
            struct Foo { x: Vec<Option<i32>>, y: i64 }
        }
        insta::assert_snapshot!(
            hex::encode(hash(&Foo {
                x: vec![None, Some(42)],
                y: 17
            })),
            @"14e42ea3d680bc815d0cea8ac20d3e872120014fb7bba8d82c3ffa7a8e6d63c41ef9631c60b73b150e3dd72efe50e8b0248321fe2b7eea09d879f3757b879372"
        );
    }

    fn hash(x: &(impl ContentHash + ?Sized)) -> digest::Output<Blake2b512> {
        blake2b_hash(x)
    }
}
