# Copy Tracking and Tracing Design

Authors: [Daniel Ploch](mailto:dploch@google.com)

**Summary:** This Document documents an approach to tracking and detecting copy
information in jj repos, in a way that is compatible with both Git's detection
model and with custom backends that have more complicated tracking of copy
information. This design affects the output of diff commands as well as the
results of rebasing across remote copies.

## Objective

Implement extensible APIs for recording and retrieving copy info for the
purposes of diffing and rebasing across renames and copies more accurately.
This should be performant both for Git, which synthesizes copy info on the fly
between arbitrary trees, and for custom extensions which may explicitly record
and re-serve copy info over arbitrarily large commit ranges.

The APIs should be defined in a way that makes it easy for custom backends to
ignore copy info entirely until they are ready to implement it.

## Interface Design

### Read API

Copy information will be served both by a new Backend trait method described
below, as well as a new field on Commit objects for backends that support copy
tracking:

```rust
/// An individual copy event, from file A -> B.
pub struct CopyRecord {
    /// The destination of the copy, B.
    pub target: RepoPathBuf,
    /// The CommitId where the copy took place.
    pub target_commit: CommitId,
    /// The source path a target was copied from.
    ///
    /// It is not required that the source path is different than the target
    /// path. A custom backend may choose to represent 'rollbacks' as copies
    /// from a file unto itself, from a specific prior commit.
    pub source: RepoPathBuf,
    pub source_file: FileId,
    /// The source commit the target was copied from. Backends may use this
    /// field to implement 'integration' logic, where a source may be
    /// periodically merged into a target, similar to a branch, but the
    /// branching occurs at the file level rather than the repository level. It
    /// also follows naturally that any copy source targeted to a specific
    /// commit should avoid copy propagation on rebasing, which is desirable
    /// for 'fork' style copies.
    ///
    /// It is required that the commit id is an ancestor of the commit with
    /// which this copy source is associated.
    pub source_commit: CommitId,
}

pub trait Backend {
    /// Get copy records for the dag range `root..head`.  If `paths` is empty
    /// include all paths, otherwise restrict to only `paths`.
    ///
    /// The exact order these are returned is unspecified, but it is guaranteed
    /// to be reverse-topological. That is, for any two copy records with
    /// different commit ids A and B, if A is an ancestor of B, A is streamed
    /// after B.
    ///
    /// Streaming by design to better support large backends which may have very
    /// large single-file histories. This also allows more iterative algorithms
    /// like blame/annotate to short-circuit after a point without wasting
    /// unnecessary resources.
    fn get_copy_records(
         &self,
         paths: &[RepoPathBuf],
         root: &CommitId,
         head: &CommitId,
    ) -> BackendResult<BoxStream<BackendResult<CopyRecord>>>;
}
```

In addition to the low-level API for directly listing `CopyRecord`s,
`CopyrecordMap` provides a higher-level API with convenient functions for
accessing `CopyRecord`s.  Conflicts between multiple copies during a merge will
be surfaced at this level of API.

```rust
/// A collection of CopyRecords.
#[derive(Default, Debug)]
pub struct CopyRecordMap { ... }

impl CopyRecordMap {
    /// Adds information about a stream of CopyRecords to `self`.  A target with
    /// multiple conflicts is discarded and treated as not having an origin.
    pub fn add_records(&mut self, stream: BoxStream<BackendResult<CopyRecord>>);

    /// Gets any copy record associated with a target path.
    pub fn for_target(&self, target: &RepoPath) -> Option<&CopyRecord>;
}
```

### Write API

Backends that support tracking copy records at the commit level will do so
through a new field on `backend::Commit` objects:

```rust
pub struct Commit {
    ...
    copies: Option<HashMap<RepoPathBuf, CopySources>>,
}

pub trait Backend {
    /// Whether this backend supports storing explicit copy records on write.
    fn supports_copy_tracking(&self) -> bool;
}
```

This field will be ignored by backends that do not support copy tracking, and
always set to `None` when read from such backends. Backends that do support copy
tracking are required to preserve the field value always.

This API will enable the creation of new `jj` commands for recording copies:

```shell
jj cp $SRC $DEST [OPTIONS]
jj mv $SRC $DEST [OPTIONS]
```

These commands will rewrite the target commit to reflect the given move/copy
instructions in its tree, as well as recording the rewrites on the Commit
object itself for backends that support it (for backends that do not,
these copy records will be silently discarded).

Flags for the first two commands will include:

```
-r/--revision
    perform the copy or move at the specified revision
    defaults to the working copy commit if unspecified
-f
    force overwrite the destination path
--after
    record the copy retroactively, without modifying the targeted commit tree
--resolve
    overwrite all previous copy intents for this $DEST
--allow-ignore-copy
    don't error if the backend doesn't support copy tracking
--from REV
    specify a commit id for the copy source that isn't the parent commit
```

For backends which do not support copy tracking, it will be an error to use
`--after`, since this has no effect on anything and the user should know that.
The `supports_copy_tracking()` trait method is used to determine this.

An additional command is provided to deliberately discard copy info for a
destination path, possibly as a means of resolving a conflict.

```shell
jj forget-cp $DEST [-r REV]
```

## Behavioral Changes

### Rebase Changes

In general, we want to support the following use cases:

-   A rebase of an edited file A across a rename of A->B should transparently move the edits to B.
-   A rebase of an edited file A across a copy from A->B should _optionally_ copy the edits to B. A configuration option should be defined to enable/disable this behavior.
-   TODO: Others?

Using the aforementioned copy tracing API, both of these should be feasible. A
detailed approach to a specific edge case is detailed in the next section.

#### Rename of an added file

A well known and thorny problem in Mercurial occurs in the following scenario:

1.  Create a new file A
1.  Create new commits on top that make changes to file A
1.  Whoops, I should rename file A to B. Do so, amend the first commit.
1.  Because the first commit created file A, there is no rename to record; it's changing to a commit that instead creates file B.
1.  All child commits get sad on evolve

In jj, we have an opportunity to fix this because all rebasing occurs atomically
and transactionally within memory. The exact implementation of this is yet to be
determined, but conceptually the following should produce desirable results:

1.  Rebase commit A from parents [B] to parents [C]
1.  Get copy records from [D]->[B] and [D]->[C], where [D] are the common ancestors of [B] and [C]
1.  DescendantRebaser maintains an in-memory map of commits to extra copy info, which it may inject into (2). When squashing a rename of a newly created file into the commit that creates that file, DescendentRebase will return this rename for all rebases of descendants of the newly modified commit. The rename lives ephemerally in memory and has no persistence after the rebase completes.
1.  A to-be-determined algorithm diffs the copy records between [D]->[B] and [D]->[C] in order to make changes to the rebased commit. This results in edits to renamed files being propagated to those renamed files, and avoiding conflicts on the deletion of their sources. A copy/move may also be undone in this way; abandoning a commit which renames A->B should move all descendant edits of B back into A.

### Conflicts

With copy-tracking, a whole new class of conflicts become possible. These need
to be well-defined and have well documented resolution paths. Because copy info
in a commit is keyed by _destination_, conflicts can only occur at the
_destination_ of a copy, not at a source (that's called forking).

#### Split conflicts

Suppose we create commit A by renaming file F1 -> F2, then we split A. What
happens to the copy info? I argue that this is straightforward:

-   If F2 is preserved at all in the parent commit, the copy info stays on the parent commit.
-   Otherwise, the copy info goes onto the child commit.

Things get a little messier if A _also_ modifies F1, and this modification is
separated from the copy, but I think this is messy only in an academic sense and
the user gets a sane result either way. If they want to separate the
modification from the copy while still putting it in an earlier commit, they can
express this intent after with `jj cp --after --from`.

#### Merge commit conflicts

Suppose we create commit A by renaming file F1 -> F, then we create a sibling
commit B by renaming file F2 -> F. What happens when we create a merge commit
with parents A and B?

In terms of _copy info_ there is no conflict here, because C does not have copy
info and needs none, but resolving the contents of F becomes more complicated.
We need to (1) identify the greatest common ancestor of A and B (D)
(which we do anyway), and (2) invoke `get_copy_records()` on F for each of
`D::A` and `D::B` to identify the 'real' source file id for each parent. If
these are the same, then we can use that as the base for a better 3-way merge.
Otherwise, we must treat it as an add+add conflict where the base is the empty
file id.

It is possible that F1 and F2 both came from a common source file G, but that
these copies precede D. In such case, we will not produce as good of a merge
resolution as we theoretically could, but (1) this seems extremely niche and
unlikely, and (2) we cannot reasonably achieve this without implementing some
analogue of Mercurial's linknodes concept, and it would be nice to avoid that
additional complexity.

#### Squash conflicts

Suppose we create commit A by renaming file F1 -> F, then we create child
commit B in which we replace F by renaming F2 -> F. This touches on two issues.

Firstly, if these two commits are squashed together, then we have a destination
F with two copy sources, F1 and F2. In this case, we can store a
`CopySources::Conflict([F1, F2])` as the copy source for F, and treat this
commit as 'conflicted' in `jj log`. `jj status` will need modification to show
this conflicted state, and `jj resolve` will need some way of handling the
conflicted copy sources (possibly printing them in some structured text form,
and using the user's merge tool to resolve them). Alternatively, the user can
'resolve directly' by running `jj cp --after --resolve` with the desired copy
info.

Secondly, who is to say that commit B is 'replacing' F at all? In some version
control systems, it is possible to 'integrate' a file X into an existing file Y,
by e.g. propagating changes in X since its previous 'integrate' into Y, without
erasing Y's prior history in that moment for the purpose of archaeology. With
the commit metadata currently defined, it is not possible to distinguish
between a 'replacement' operation and an 'integrate' operation.

##### Track replacements explicitly

One solution is to add a `destructive: bool` field or similar to the
`CopySource` struct, to explicitly distinguish between these two types of copy
records. It then becomes possible to record a non-destructive copy using
`--after` to recognize that a file F was 'merged into' its destination, which
can be useful in handling parallel edits of F that later sync this information.

##### Always assume replacement

Alternatively, we can keep copy-tracking simple in jj by taking a stronger
stance here and treating all copies-onto-existing-files as 'replacement'
operations. This makes integrations with more complex VCSs that do support
'integrate'-style operations trickier, but it is possible that a more generic
commit extension system is better suited to such backends.

### Future Changes

An implementation of `jj blame` or `jj annotate` does not currently exist, but
when it does we'll definitely want it to be copy-tracing aware to provide
better annotations for users doing archaeology. The Read APIs provided are
expected to be sufficient for these use cases.

## Non-goals

### Tracking copies in Git

Git uses rename detection rather than copy tracking, generating copy info on
the fly between two arbitrary trees. It does not have any place for explicit
copy info that _exchanges_ with other users of the same git repo, so any
enhancements jj adds here would be local only and could potentially introduce
confusion when collaborating with other users.

### Directory copies/moves

All copy/move information will be read and written at the file level. While
`jj cp|mv` may accept directory paths as a convenience and perform the
appropriate tree modification operations, the renames will be recorded at the
file level, one for each copied/moved file.
