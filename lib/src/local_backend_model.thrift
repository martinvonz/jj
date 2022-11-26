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

struct File {
  1: required binary id,
  2: required bool executable,
}

union TreeValue {
  1: File file,
  2: binary symlink_id,
  3: binary tree_id,
  4: binary conflict_id,
}

struct TreeEntry {
  1: required string name,
  2: required TreeValue value,
}

struct Tree {
  1: required list<TreeEntry> entries,
}

struct Timestamp {
  1: required i64 millis_since_epoch,
  2: required i32 tz_offset,
}

struct Signature {
  1: required string name,
  2: required string email,
  3: required Timestamp timestamp,
}

struct Commit {
  1: required list<binary> parents,
  2: required list<binary> predecessors,
  3: required binary root_tree,
  4: required binary change_id,
  5: required string description,
  6: required Signature author,
  7: required Signature committer,
}

struct ConflictPart {
  1: required TreeValue content,
}

struct Conflict {
  1: required list<ConflictPart> removes,
  2: required list<ConflictPart> adds,
}
