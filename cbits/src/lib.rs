// Copyright 2020 The Jujutsu Authors
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

//! ## Dark Arts of the Jujutsu Version Control System
//!
//! This crate contains internal and third-party C code used by Jujutsu. Exposed
//! with 'mid-level' Rust bindings. This crate **MUST NOT** export `unsafe` APIs
//! to the rest of the Jujutsu codebase (we use `deny(unsafe_code)`). Instead,
//! it is an attempt to wrangle some unneeded unsafe code into a place where it
//! can be audited and scrutinized.
//!
//! This crate is designed **ONLY** for use with the `jj_*` family of crates. It
//! is useful not just for the developers, but some client crate uses, e.g.
//! setting the memory allocator for the entire process.
//!
//! If you are reading this, and you are not a Jujutsu developer or using
//! Jujutsu crates, be warned:
//!
//! - It may or may not be useful to you (probably not), and
//! - You should not consider this to be a stable or usable API for any uses
//! except Jujutsu-related ones, i.e. clients of the various other `jj_*`
//! crates. The version of this crate and the version of Jujutsu itself are
//! intimately tied. And finally,
//! - If you are just looking for bindings to your favorite C libraries, this is
//! not where you want to be.
//! - You could contact us and chat, or maybe fork the code if you can mold it
//! to your liking? It's up to you.
//!
//! Assuming you are a Jujutsu developer, if you are reading this, **DO NOT ADD
//! CODE TO THIS CRATE UNLESS YOU KNOW WHAT YOU ARE DOING**. In order for your
//! code to be accepted here, in the ancestral homeland of the devil, you must
//! pass an ancient ritual fortold in near-forgetten scrolls and come out
//! unscathed. The specifics of this ritual, however, have been forgotten. Also,
//! it will have to go through significant code review.
//!
//! It is possible the code in this crate may one day become usable to a wider
//! world. Or not. It is also possible that code within may be replaced or
//! modified. Please tread carefully. Do not taunt Happy Fun Ball.
//!
//! At this time, **this library is `no_std`**! It is not designed to host
//! 'traditional' Rust code, only thin 'mid-level' FFI layers. This restriction
//! may be lifted in the future if it is required for better interoperability
//! and safer APIs.

// Disable std support to avoid recursive dependencies, since this library
// contains an allocator.
#![no_std]

#[cfg(feature = "mimalloc")]
pub mod mimalloc;
