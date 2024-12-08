# Copy Tracking and Tracing Design

Authors: [Daniel Ploch](mailto:dploch@google.com)

**Summary:** This Document documents an approach to tracking and detecting copy
information in jj repos, in a way that is compatible with both Git's detection
model and with custom backends that have more complicated tracking of copy
information. This design affects the output of diff commands as well as the
results of rebasing across remote copies.

## Objective

Add support for copy information that is sufficient for at least the following
use cases:

* Diffing: If a file has been copied, show a diff compared to the source version
  instead of showing a full addition.
* Merging: When one side of a merge (or rebase) has copied a file and the other
  side has modified it, propagate the changes to the other side. (There are many
  other case to handle too.)
* Log: It should be possible to run something like `jj log -p <file>` and follow
  the file backwards when it had been created by copying.
* Annotate (blame): Similar to the log use case, we should follow the file
  backwards when it had been created by copying.

The solution should support recording and retrieving copy info in a way that
is performant both for Git, which synthesizes copy info on the fly between
arbitrary trees, and for custom backends which may explicitly record and
re-serve copy info over arbitrarily large commit ranges.

The APIs should be defined in a way that makes it easy for custom backends to
ignore copy info entirely until they are ready to implement it.

### Desired UX

The following sections describe some scenarios and how we would ideally handle
them.

#### Restoring from a commit should preserve copies

For example, `jj new X--; jj restore --from X` should restore any copies
made in `X-` and `X` into the new working copy. Transitive copies should
be "flattened". For example, if `X-` renamed `foo` to `bar` and `X` renamed
`bar` to `baz`, then the restored commit should rename `foo` to `baz`.

This also applies to reparenting in general, such as for
["verbatim rebase"](https://github.com/martinvonz/jj/issues/1027).

#### Diff after restore

`jj restore --from X; jj diff --from X` should be empty, at least when it comes
to file contents. It may indicate that renamed file have different history.

#### Lossless round-trip of rebase

Except for the [`A+(A-B)=A` rule][same_change_rule], rebasing is currently never
lossy; rebasing a commit and then rebasing it back yields the same content. We
should ideally preserve this property when possible.

For example:
```
$ jj log
C rename bar->baz
|
B rename foo->bar
|
A add foo
$ jj rebase -r C -d A
$ jj rebase -r C -d B # Takes us back to the state above
```


#### Backing out the parent commit should be a no-op

Patches should be reversible so you can make a change and then back it out, and
end up with an empty diff across both commits.

For example:
```
$ jj log
B rename foo->bar
|
A add foo
$ jj backout -r B -d B
$ jj diff --from B- --to B+ # Should be empty
```

#### Parallelize/serialize

This is a special case of the lossless rebase.
```
$ jj log
E edit qux
|
D rename baz->qux
|
C rename bar->baz
|
B rename foo->bar
|
A add foo
$ jj parallelize B::D
# There should be no conflict in E and it should look like a
# regular edit just like before
$ jj rebase -r C -A B
$ jj rebase -r D -A C
# Now we're back to the same graph as before.
```

#### Copies inside merge commit

We should be able to resolve a naming conflict:
```
$ jj log
D  resolve naming conflict by choosing `foo` as the source
|\
C | rename bar->baz
| |
| B rename foo->baz
|/
A add foo and bar
$ jj file annotate baz # Should not include changes from C
```

We should also be able to back out that resolution and get back into the
name-conflicted state.

We should be able to rename files that exist on only one side:
```
$ jj log
D  rename foo2->foo3 and bar2->bar3
|\
C | rename bar->bar2
| |
| B rename foo->foo2
|/
A add foo and bar
```

#### Copies across merge commit

```
$ jj log
D delete baz
|\
C | rename foo->baz
| |
| B rename foo->bar
|/
A add foo
```

`jj diff --from C --to D` should now show a baz->bar rename (just like
`jj diff --from C --to B` would). `jj diff --from B --to D` should show
no renames. That's despite there being a rename in C.

## High-level Design

Jujutsu uses a snapshot-based model similar to Git's. The algebra for our
first-class conflicts is also based on snapshots and being able to calculate
patches as differences between states. That means that we have to fit copy
information into that snapshot-based model too[^martinvonz_slow].

The proposal is to update tree objects to also contain information about a
file's past names. For example, if file `foo` gets renamed to `bar` in one
commit and then to `baz` in another commit, we will record that `baz` previously
had names `bar` and `foo`.

To support merging two files into one, the list of past names is actually a DAG.
Merging can happen in a merge commit when two sides copy/rename different
source files to the same target file. By having support for it in the model, we
can also support merging multiple files into one in a regular non-merge commit.

To avoid having to store all past paths in the tree object entry, we will write
the copy history as an object and the tree will refer to the object by ID.

If file `foo` gets renamed to `bar`, and then file `bar` gets rewritten from
scratch, we can indicate that by resetting its copy graph. Do we also want to
support resetting a file that was *not* renamed to a new graph to indicate that
it's been rewritten from scratch? That would mean that we have different empty
graphs. Perhaps generate a random ID identifying a file? But then what does it
mean if the same ID exists in at multiple paths (even in the same tree object)?

Do we need to store the names in the DAG at all? Is the ID enough? The model
becomes quite similar to BitKeeper's model, which has assigns a file ID that
identifies a file across renames (but not copies). However, a significant
difference is that our copy ID will change when a file is renamed/copied and
we instead keep a pointer to the previous ID.

Having the path in the copy graph can be useful for finding copy sources without
having to scan the whole tree or having to ask the backend. If we use the file
name as only input to the ID, then we also get deterministic tree ids. But then
we don't get the option to say that a file gets a new ID without renaming.

Do we want to allow an edge in the copy graph to be able to say that changes
should not be propagated across it? It might be useful to still have such edges
for the log and annotation use cases.

Only *file* entries in the tree have a copy ID. We won't support tracking copied
symlinks or directories.

The data structure might look like this:
```rust
// Current `TreeValue::File` variant:
File { id: FileId, executable: bool },
// New `TreeValue::File` variant:
File { id: FileId, executable: bool, copy_id: CopyId },

// A CopyId is a hash of this struct:
struct CopyHistory {
    path: RepoPath,
    parents: Vec<CopyId>
}
```


### Diffing

When diffing two trees, we first diff the trees without considering copy info.
If any copy IDs changed in that diff, we walk the copy graph backwards until
we find it on the opposite side of the diff. If we find it there, it indicates
that the file was copied from/to the other side.

Let's look at an example of how this model would look this scenario:

```
$ jj log
M rename bar->baz, set baz="M"
|
L copy foo->bar, set bar="L"
|
K add foo="K"
```

The trees would look like this:
```
Commit K:
name: foo, id: aaa111, copy_id: 1:foo

Commit L:
name: bar, id: aaa111, copy_id: 2:bar->1:foo
name: foo, id: aaa111, copy_id: 1:foo

Commit M:
name: baz, id: aaa111, copy_id: 3:baz->2:bar->1:foo
name: foo, id: aaa111, copy_id: 1:foo
```

When diffing from `K` to `M` without considering copy info, we see that `baz`
was created. To find which files were copied/renamed, we compare all changed
`copy_id`s. In this case, we find that `3:baz` was added. We then follow the
copy graph for added IDs backwards until we find a name and ID that matches
what we have in the source tree. In this case, `bar` doesn't exist in commit
`K`, but `foo` does exist and its ID matches the `1:foo` we found in the file
ID graph.

When diffing from `L` to `M` we would find that `bar` had been removed and that
`baz` had been added. As before, we walk the copy graph backwards. In this case
we find that `2:bar` exists in `L`, so we use that as the left side of the diff
for `baz`. Even though `1:foo` also exists in `L`, we use `2:bar` since that's
the closer ancestors in the copy graph.

When diffing from `M` to `K` (i.e. in the other direction compared to the cases
above), we find that `baz` was removed. To find which files were copied/renamed, we
compare all changed `copy_id`s. We walk the `baz`'s graph backwards to find
that `1:foo` exists in `K`.


```
$ jj log
M copy foo->bar
|
L delete bar
|
K add foo, bar
```

```
$ jj log
M rename foo->baz, create bar
|
| L rename foo->bar, create baz
|/
K add foo
```

```
$ jj log
M copy foo->baz, create bar
|
| L copy foo->bar, create baz
|/
K add foo
```

```
$ jj log
M copy foo->bar, foo="M", bar="M2"
|
| L copy foo->bar, foo="L", bar="L2"
|/
K add foo="K"
```

### Merging

When merging trees, we start by rewriting each diff to match any different names
in the destination tree. For example, if the tree conflict is `A+(B-C)+(D-E)`,
then we will rewtite the `(B-C)` diff and the `(D-E)` diff to the paths in `A`.
To translate the `(B-C)` diff, we calculate renames from `C` to `A` and then we
apply those renames to both `C` and `B`. This may result in conflicts.

If a file has a conflict in the copy ID, it will appear as if doesn't exist when
materialized. It will therefore not show up the working copy until the user has
resolved the conflict.

Should we propagate changes to copies? Mercurial does that but I feel like I've
almost never wanted it. It has been annoying quite a few times, but maybe I just
don't notice it when it's what I wanted.

For example:
```
Z set foo="bye"
|
| Y rename foo->bar
|/
X add foo="hello"
```


```
$ jj log
N rename baz->baz2, modify foo
| 
| M rename bar->bar2, modify baz
| |
| | L rename foo->foo2, modify bar
| |/
|/
K add foo, bar, baz
$ jj new L M N
```


#### Convergent renames

Consider this "convergent copy/rename" scenario:
```
$ jj log
C rename bar->baz
|
| B rename foo->baz
|/
A add foo, add bar
$ jj new B C
```

It seems clear that `baz`'s copy graph should inherit from both `foo` and `bar`,
producing a merge in copy graph. The trees would look like this:
```
Commit A:
name: foo, id: aaa111, copy_id: 1:foo
name: bar, id: aaa111, copy_id: 2:bar

Commit B:
name: bar, id: aaa111, copy_id: 2:bar
name: baz, id: aaa111, copy_id: 3:baz->1:foo

Commit C:
name: foo, id: aaa111, copy_id: 1:foo
name: baz, id: aaa111, copy_id: 4:baz->2:bar

Merge commit:
name: baz, id: aaa111, copy_id: 5:baz->{3:baz->1:foo,4:baz->2:bar}
```

We used the same content for both `foo` and `bar` above to simplify. If they
had been different, we would have had a conflict in the contents but the copy
ID would still have been clear.

#### Rebasing


```
$ jj log
C rename bar->baz
|
B rename foo->bar
|
A add foo
$ jj rebase -r C -d A
```


```
$ jj log
C rename foo->baz
|
| B rename foo->bar
|/
A add foo
$ jj rebase -r C -d B
```


#### Divergent renames

Consider this "divergent rename" scenario:
```
$ jj log
C rename foo->baz
|
| B rename foo->bar
|/
A add foo
$ jj new B C
```

In this scenario, the regular 3-way merge of the trees without considering copy
info results in a tree without conflicts. However, the user might reasonably
expect to have to choose between the `bar` and `baz` names. Here's what Git says
in this scenario:

```
$ git merge main
CONFLICT (rename/rename): foo renamed to baz in HEAD and to bar in main.
Automatic merge failed; fix conflicts and then commit the result.

$ git st
HEAD detached from ab0b8e3
You have unmerged paths.
  (fix conflicts and run "git commit")
  (use "git merge --abort" to abort the merge)

Unmerged paths:
  (use "git add/rm <file>..." as appropriate to mark resolution)
        added by them:   bar
        added by us:     baz
        both deleted:    foo
```

Interestingly, Git seems to represent this state by using index states that
would not normally end up in the index as a result of conflicts.

Here's what Mercurial says:

```
$ hg merge main
note: possible conflict - foo was renamed multiple times to:
 bar
 baz
1 files updated, 0 files merged, 0 files removed, 0 files unresolved
(branch merge, don't forget to commit)
```

Mercurial doesn't have a place to record this state, so it just prints that
note and leaves it at that.

The model and algorithm described in this document would result in a conflict
in the copy ID at both paths after propagating the renames.

#### @jonathantanmy's test case:

```
$ jj log
E baz="baz" (resolves conflict)
|
D <conflict>
|\
C | rename bar->baz
| |
| B rename foo->baz
|/
A add foo="foo" and bar="bar"
$ jj rebase -r E -d C
$ jj rebase new D E -m F
```

If F is empty (auto-merged), it should have the same state as E before.


### Log

The copy graph contains all past paths and copy IDs of a file, so when doing
`jj log <filename>`, we might want to translate that to a revset that's similar
to `files()` but matches specific (path, copy ID) pairs instead of specific
paths.

### Annotate


### Representation in Git

Do we ever want to record renames in the Git backend? If we do, we would
presumably store it outside the Git object, similar to how we store the change
id for commits.

What do we use for trees where we *don't* have any copy graph recorded? If we
simply create a new copy graph based on the current path, then the caller will
never find any copies. Do we need an indexing pass to detect all renames in a
repo when running `jj git init`? That can be very expensive for large repos.
For reference, `git log --summary --find-copies-harder` takes about 165 seconds
in the git.git repo on my computer.

How to deal with two trees having the same content but different file ids?
Actually store the additional data linked from the commit object? That would
not work if we point to trees from somewhere that's not a commit. We point to a
tree from the working-copy state.

One could imagine not storing any copy info in Git and instead making the model
described above an implementation detail of the backend. Then it could be used
by the native backend and the Google backend, while we still use on-the-fly
copy detection in the Git backend. However, if we want to be able to tell the
user about details of conflicting copy IDs so they can decide how to resolve
such conflicts, then we would have to somehow represent that abstractly too.

### Representation in cloud repo (e.g. Google)

Let's say you have a commit with some files you've modified. You now want to
sync (rebase) that to an updated main branch. If some of the files you modified
no longer exist on the main branch, we want to figure out if they were renamed
so we should propagate your changes to the new file location. As described
earlier, we can do that by finding files that have a different copy id since the
last time you synced. However, if there are 10 million new commits on the main
branch, there's perhaps tens of thousands of such files spread across the entire
tree. That can therefore can be very expensive to calculate. We therefore need
to be able to get help from a custom backend implementation with this query.

Since the conflict is at that tree level and we want to reproduce the conflicted
tree on the fly like we already do for file content, I hope we can get the
backend query to not require a commit id. Hopefully it can take just two root
trees (source and destination) and a set of paths and return the copy targets.

So, let's say we have a method on the commit backend for finding copy targets
for a set of paths between two given trees. How would the cloud-based backend
implement that?

One option might be for it to keep an index of all copy targets for a given copy
source. Then it can look up the possible targets for the requested source files
and then check which of them were actually copied from the source tree to the
destination tree.

A weakness of this solution is that the search gets expensive if there are very
many related files. That's probably not much of a problem in practice. The
server might want to populate the the index only for public/immutable commits.
Otherwise, a user could poison the index by creating tons of copyies
(intentionally or by mistake), which would make all future queries about those
files expensive.

## Interface Design


### Read API

Copy information will be served both by a new Backend trait method described
below, as well as a new field on Commit objects for backends that support copy
tracking:

```rust
/// An individual copy source.
pub struct CopySource {
    /// The source path a target was copied from.
    ///
    /// It is not required that the source path is different than the target
    /// path. A custom backend may choose to represent 'rollbacks' as copies
    /// from a file unto itself, from a specific prior commit.
    path: RepoPathBuf,
    file: FileId,
    /// The source commit the target was copied from. If not specified, then the
    /// parent of the target commit is the source commit. Backends may use this
    /// field to implement 'integration' logic, where a source may be
    /// periodically merged into a target, similar to a branch, but the
    /// branching occurs at the file level rather than the repository level. It
    /// also follows naturally that any copy source targeted to a specific
    /// commit should avoid copy propagation on rebasing, which is desirable
    /// for 'fork' style copies.
    ///
    /// If specified, it is required that the commit id is an ancestor of the
    /// commit with which this copy source is associated.
    commit: Option<CommitId>,
}

pub enum CopySources {
    Resolved(CopySource),
    Conflict(HashSet<CopySource>),
}

/// An individual copy event, from file A -> B.
pub struct CopyRecord {
    /// The destination of the copy, B.
    target: RepoPathBuf,
    /// The CommitId where the copy took place.
    id: CommitId,
    /// The source of the copy, A.
    sources: CopySources,
}

/// Backend options for fetching copy records.
pub struct CopyRecordOpts {
    // TODO: Probably something for git similarity detection
}

pub type CopyRecordStream = BoxStream<BackendResult<CopyRecord>>;

pub trait Backend {
    /// Get all copy records for `paths` in the dag range `roots..heads`.
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
    async fn get_copy_records(&self, paths: &[RepoPathBuf], roots: &[CommitId], heads: &[CommitId]) -> CopyRecordStream;
}
```

Obtaining copy records for a single commit requires first computing the files
list for that commit, then calling get_copy_records with `heads = [id]` and
`roots = parents()`. This enables commands like `jj diff` to produce better
diffs that take copy sources into account.

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

-   A rebase of an edited file A across a rename of A->B should transparently
    move the edits to B.
-   A rebase of an edited file A across a copy from A->B should _optionally_
    copy the edits to B. A configuration option should be defined to
    enable/disable this behavior.
-   TODO: Others?

Using the aforementioned copy tracing API, both of these should be feasible. A
detailed approach to a specific edge case is detailed in the next section.

#### Rename of an added file

A well known and thorny problem in Mercurial occurs in the following scenario:

1.  Create a new file A
1.  Create new commits on top that make changes to file A
1.  Whoops, I should rename file A to B. Do so, amend the first commit.
1.  Because the first commit created file A, there is no rename to record; it's
    changing to a commit that instead creates file B.
1.  All child commits get sad on evolve

In jj, we have an opportunity to fix this because all rebasing occurs atomically
and transactionally within memory. The exact implementation of this is yet to be
determined, but conceptually the following should produce desirable results:

1.  Rebase commit A from parents [B] to parents [C]
1.  Get copy records from [D]->[B] and [D]->[C], where [D] are the common
    ancestors of [B] and [C]
1.  MutableRepo maintains an in-memory map of commits to extra copy info,
    which it may inject into (2). When squashing a rename of a newly created
    file into the commit that creates that file, MutableRepo will return
    this rename for all rebases of descendants of the newly modified commit. The
    rename lives ephemerally in memory and has no persistence after the rebase
    completes.
1.  A to-be-determined algorithm diffs the copy records between [D]->[B] and
    [D]->[C] in order to make changes to the rebased commit. This results in
    edits to renamed files being propagated to those renamed files, and avoiding
    conflicts on the deletion of their sources. A copy/move may also be undone
    in this way; abandoning a commit which renames A->B should move all
    descendant edits of B back into A.

### Conflicts

With copy-tracking, a whole new class of conflicts become possible. These need
to be well-defined and have well documented resolution paths. Because copy info
in a commit is keyed by _destination_, conflicts can only occur at the
_destination_ of a copy, not at a source (that's called forking).

#### Split conflicts

Suppose we create commit A by renaming file F1 -> F2, then we split A. What
happens to the copy info? I argue that this is straightforward:

-   If F2 is preserved at all in the parent commit, the copy info stays on the
    parent commit.
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


## Alternatives considered

### Detect copies (like Git)

Git doesn't record copy info. Instead, it infers it when comparing two trees.

It's hard to make this model scale to very large repos. For example, let's say
you're rebasing your local commit to a new upstream commit that's 1 million
commits ahead. We would then want to find if any of the files in your local
commit has been copied upstream. That's very expensive to do by comparing the
old and the new base trees. However, since the query APIs defined above take
commits (not trees) as input, we allow the backend to take the history into
account when calculating the copies. A backend can then create an index based
on the input files (in your local commit) and find if it's been copied without
comparing the full trees.

### Record logical file identifiers in trees (BitKeeper-like model)

BitKeeper records a file ID (which identifies a logical file, unlike our `FileId`
type) for each path (or maybe it's a path for each file ID). That way you can
compare two arbitrary trees, find the added and deleted files and just compare
the file IDs to figure out which of them are renames.

This model doesn't seem to be easily extensible to support copies (it only
supports renames).

To perform a rebase across millions of commits, we would not want to diff the
full trees because that would be too expensive (probably millions of modified
files). We could perhaps instead find renames by bisecting to find commits that
deleted any of the files modified in the commit we're rebasing.

Another problem is how to synthesize the file IDs in the Git backend. That could
perhaps be done by walking from the root commits and persisting an index.

### Include copy info in the FileId (Mercurial-like model)

Mercurial stores copy info in a metadata section in the file content itself
[^mercurial_changeset_copies]. That means that a file will get a new file
(content) ID if its copy history changes. That's quite similar to the proposal
in this document. One difference is that Mercurial's model stores information
only about the most recent copy. If the file is then modified, it will get a new
file ID. One therefore has to walk the history of the file to find the previous
name (which is usually not much of a problem because Mercurial stores a revision
DAG per file in addition to the revision DAG at the commit level).

### Hybrid snapshot/patch model with copy info stored in commits

We considered storing copy info about the copies/renames in the commit object.
That has some significant impact on the data model:

* Without copy info, if there's a linear chain of commits A..D, you can find
  the total diff by diffing just D-A. That works because (B-A)+(C-B)+(D-C)
  simplifies to just D-A. However, if there is copy info, the total diff will
  involve copy info. If that's associated with the individual commits, we will
  need to aggregate it somehow.
* Restoring from another tree is no longer just a matter of copying that tree;
  we also need to figure out copies between the old tree and the new tree.
* Conflict states are represented by a series of states to add and remove. This
  does not work with the patch-based copy info. We spent a lot of time trying
  to figure out a solution that works, but it seems like the snapshot-based
  conflict model and the patch-based copy info model are not reconcilable.
  Therefore, we won't track conflicted copy info, such as between a `foo`->`baz`
  rename and a `bar`->`baz` rename.
* Since copy records are relative to the auto-merged parents, that unfortunately
  means that the records will depend on the merge algorithm, so it's possible
  that a future change to the merge algorithm will make some copy records
  invalid. We will therefore need to not assume that the copy source exists.

For the state in conflicted commits, we considered using a representation like
this:

```rust
struct MergedTree {
    snapshot: Tree,
    diffs: Diff
}

struct Diff {
    before: Tree,
    after: Tree,
    /// Copies from `before` to `after`
    copies: Vec<CopyInfo>,
    /// Copies from `before` to `snapshot`
    copies_to_snapshot: Vec<CopyInfo>,
}

struct CopyInfo {
    source: RepoPathBuf,
    target: RepoPathBuf,
    // Maybe more fields here for e.g. "do not propagate"
}
```

That works for calculating the resulting tree, but it does not seem to allow for
doing the conflict algebra we currently do. That means that things like
parallelizing commits and then serializing them again would lose copy
information.


[same_change_rule]: https://github.com/martinvonz/jj/blob/560d66ecee5a9904b42dbc0b89333f0c27c683de/lib/src/merge.rs#L98-L111

[^martinvonz_slow]: This took me (@martinvonz) months to really understand.
[^mercurial_changeset_copies]: From around
    https://repo.mercurial-scm.org/hg/rev/49ad315b39ee, Mercurial also supports
    storing copy info in commits. That made it the kind of snapshot/patch model
    we described above as not working well.
