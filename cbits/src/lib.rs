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

//! Internal and third-party C code used by the Jujutsu version control system.
//! Exposed with 'mid-level' Rust bindings; these APIs are intended to be
//! type-safe and not require `unsafe`. Designed solely for use with the `jj_*`
//! family of crates.
//!
//! All C code that can be isolated to the `jj_cbits` crate is done so in order
//! to avoid polluting all other users with `unsafe`, and to give a central
//! location for security audits and dependency scrutiny for critical code. If
//! you are reading this, and you are not a Jujutsu developer or using Jujutsu
//! crates, be warned:
//!
//! - It may or may not be useful to you (probably not), and
//! - You should not consider this to be a stable or usable API for any uses
//! except Jujutsu-related ones, i.e. clients of the various other `jj_*`
//! crates. The version of this crate and the version of Jujutsu itself are
//! intimately tied. And finally,
//! - If you are just looking for bindings to your favorite C libraries, this is
//! not where you want to be.
//!
//! Assuming you are a Jujutsu developer, if you are reading this, DO NOT ADD
//! CODE TO THIS CRATE UNLESS YOU KNOW WHAT YOU ARE DOING. YOU DO NOT KNOW WHAT
//! YOU ARE DOING. THIS CRATE DOES NOT ACCEPT ANYTHING BUT CRITICALLY NECESSARY,
//! YET UNSAFE CODE. THIRD PARTY DEPENDENCIES ARE HELD TO EXCEPTIONALLY HIGH
//! SECURITY, DESIGN, AND PRODUCTION STANDARDS. THERE IS A HIGH LIKELIHOOD YOUR
//! CHANGE WILL BE REJECTED. UNSAFE CODE IS UNWELCOME, BUT IF YOU MUST WRITE IT,
//! IT IS BETTER TO USE RUST TO PERFORM SUCH A TASK, FOR IMPROVED BUILD,
//! INTEGRATION, AND DEVELOPER TOOLING. DO NOT WRITE C/C++ CODE AND INCLUDE IT
//! HERE IN ANYTHING BUT THE MOST DIRE CIRCUMSTANCES. UNSAFE CODE IS UNWELCOME.
//! UNSAFE CODE IS UNWELCOME. UNSAFE CODE IS UNWELCOME. UNSAFE CODE IS
//! UNWELCOME. UNSAFE CODE IS UNWELCOME.
//!
//! **DO NOT TAUNT HAPPY FUN BALL. YOU HAVE BEEN WARNED.**
//!
//! At this time, **this library is `no_std`**! It is not designed to host
//! 'traditional' Rust code, only thin 'mid-level' FFI layers. This restriction
//! may be lifted in the future if it is required for better interoperability
//! and safer APIs.

// Disable std support to avoid recursive dependencies, since this library
// contains an allocator.
#![no_std]

pub mod mimalloc;
