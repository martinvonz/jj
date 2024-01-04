// Copyright 2020-2024 The Jujutsu Authors
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

pub trait ObjectId {
    fn new(value: Vec<u8>) -> Self;
    fn object_type(&self) -> String;
    fn from_bytes(bytes: &[u8]) -> Self;
    fn as_bytes(&self) -> &[u8];
    fn to_bytes(&self) -> Vec<u8>;
    fn from_hex(hex: &str) -> Self;
    fn hex(&self) -> String;
}

macro_rules! id_type {
    ($vis:vis $name:ident) => {
        content_hash! {
            #[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Hash)]
            $vis struct $name(Vec<u8>);
        }
        $crate::object_id::impl_id_type!($name);
    };
}

macro_rules! impl_id_type {
    ($name:ident) => {
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
                f.debug_tuple(stringify!($name)).field(&self.hex()).finish()
            }
        }

        impl crate::object_id::ObjectId for $name {
            fn new(value: Vec<u8>) -> Self {
                Self(value)
            }

            fn object_type(&self) -> String {
                stringify!($name)
                    .strip_suffix("Id")
                    .unwrap()
                    .to_ascii_lowercase()
                    .to_string()
            }

            fn from_bytes(bytes: &[u8]) -> Self {
                Self(bytes.to_vec())
            }

            fn as_bytes(&self) -> &[u8] {
                &self.0
            }

            fn to_bytes(&self) -> Vec<u8> {
                self.0.clone()
            }

            fn from_hex(hex: &str) -> Self {
                Self(hex::decode(hex).unwrap())
            }

            fn hex(&self) -> String {
                hex::encode(&self.0)
            }
        }
    };
}

pub(crate) use {id_type, impl_id_type};
