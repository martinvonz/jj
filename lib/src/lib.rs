// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

//! Jujutsu version control system.

#![warn(missing_docs)]
#![deny(unused_must_use)]
#![forbid(unsafe_code)]

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
pub mod file_util;
pub mod files;
pub mod fmt_util;
pub mod fsmonitor;
pub mod git;
pub mod git_backend;
pub mod gitignore;
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
