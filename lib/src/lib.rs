// Copyright 2020 Google LLC
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

#![feature(get_mut_unchecked)]
#![feature(map_first_last)]

#[cfg(test)]
#[macro_use]
extern crate maplit;

pub mod commit;
pub mod commit_builder;
pub mod conflicts;
pub mod dag_walk;
pub mod evolution;
pub mod files;
pub mod git;
pub mod git_store;
pub mod index;
pub mod index_store;
pub mod local_store;
pub mod lock;
pub mod matchers;
pub mod op_heads_store;
pub mod op_store;
pub mod operation;
pub mod protos;
pub mod repo;
pub mod repo_path;
pub mod rewrite;
pub mod settings;
pub mod simple_op_store;
pub mod store;
pub mod store_wrapper;
pub mod testutils;
pub mod transaction;
pub mod tree;
pub mod tree_builder;
pub mod trees;
pub mod view;
pub mod working_copy;
