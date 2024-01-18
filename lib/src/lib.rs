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

//! Jujutsu version control system.

#![warn(missing_docs)]
#![deny(unused_must_use)]
#![forbid(unsafe_code)]

// Needed so that proc macros can be used inside jj_lib and by external crates
// that depend on it.
// See:
// - https://github.com/rust-lang/rust/issues/54647#issuecomment-432015102
// - https://github.com/rust-lang/rust/issues/54363
extern crate self as jj_lib;

#[macro_use]
pub mod content_hash;

pub mod backend;
pub mod commit;
pub mod commit_builder;
pub mod conflicts;
pub mod dag_walk;
pub mod default_index;
pub mod default_submodule_store;
pub mod diff;
pub mod extensions_map;
pub mod file_util;
pub mod files;
pub mod fmt_util;
pub mod footer;
pub mod fsmonitor;
pub mod git;
pub mod git_backend;
pub mod gitignore;
pub mod gpg_signing;
pub mod hex_util;
pub mod id_prefix;
pub mod index;
pub mod local_backend;
pub mod local_working_copy;
pub mod lock;
pub mod matchers;
pub mod merge;
pub mod merged_tree;
pub mod object_id;
pub mod op_heads_store;
pub mod op_store;
pub mod op_walk;
pub mod operation;
#[allow(missing_docs)]
pub mod protos;
pub mod refs;
pub mod repo;
pub mod repo_path;
pub mod revset;
pub mod revset_graph;
pub mod rewrite;
pub mod settings;
pub mod signing;
pub mod simple_op_heads_store;
pub mod simple_op_store;
pub mod ssh_signing;
pub mod stacked_table;
pub mod store;
pub mod str_util;
pub mod submodule_store;
pub mod transaction;
pub mod tree;
pub mod tree_builder;
pub mod view;
pub mod working_copy;
pub mod workspace;
