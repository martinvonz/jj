// Copyright 2022 The Jujutsu Authors
//
// Licensed under the Apache License, Version 2.0 (the "License"),
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

struct RefConflict {
  1: required list<binary> removes,
  2: required list<binary> adds,
}

union RefTarget {
  1: binary commit_id,
  2: RefConflict conflict,
}

struct RemoteBranch {
  1: required string remote_name,
  2: required RefTarget target,
}

struct Branch {
  1: required string name,
  // Unset if the branch has been deleted locally.
  2: optional RefTarget local_target,
  // TODO: How would we support renaming remotes while having undo work? If
  // the remote name is stored in config, it's going to become a mess if the
  // remote is renamed but the configs are left unchanged. Should each remote
  // be identified (here and in configs) by a UUID?
  3: required list<RemoteBranch> remote_branches,
}

struct GitRef {
  1: required string name,
  2: required RefTarget target,
}

struct Tag {
  1: required string name,
  2: required RefTarget target,
}

struct View {
  1: required list<binary> head_ids,
  2: required list<binary> public_head_ids,
  3: required map<string, binary> wc_commit_ids,
  4: required list<Branch> branches,
  5: required list<Tag> tags,
  // Only a subset of the refs. For example, does not include refs/notes/.
  6: required list<GitRef> git_refs,
  7: optional binary git_head,
}

struct Operation {
  1: required binary view_id,
  2: required list<binary> parents,
  3: required OperationMetadata metadata,
}

// TODO: Share with store.proto? Do we even need the timezone here?
struct Timestamp {
  1: required i64 millis_since_epoch,
  2: required i32 tz_offset,
}

struct OperationMetadata {
  1: required Timestamp start_time,
  2: required Timestamp end_time,
  3: required string description,
  4: required string hostname,
  5: required string username,
  6: required map<string, string> tags,
}
