# Topics

Authors: [Philip Metzger](mailto:philipmetzger@bluewin.ch), [Noah Mayr](mailto:dev@noahmayr.com)
 [Anton Bulakh](mailto:him@necaq.ua)

## Summary

Introduce Topics as a truly Jujutsu native way for topological branches, which 
also replace the current bookmark concept for Git interop. As they have been
documented to be confusing users coming from Git. They also supersede the 
`[experimental-advance-branches]` config for those who currently use it, as 
such a behavior will be built-in for Topics.

Topics have been discussed heavily since their appearance in 
[this Discussion][gh-discuss]. As Noah, Anton and I had a long 
[Discord discussion][dc-thread] about them, which then also poured into the 
[Topic issue][issue].

## Prior work 

Currently there only is Mercurial which has a implementation of 
[Topics][hg-topics]. There also is the [Topic feature][gerrit-topics] in Gerrit,
which groups commits with a single identifier.


## Goals and non-goals

### Goals

The goals for this Project are small, see below.

* Introduce the concept of native topological branches for Jujutsu.
* Simplify Git interop by reducing the burden on Bookmarks.
* Add Change metadata as a storage concept.
* Remove the awkward `bookmark` to Git `branch` mapping.

### Non-Goals

* Making bookmarks unnecessary.

## Overview

Until now, Jujutsu had no native set of topological branches, just 
[Bookmarks][bm] which interact poorly with Git's expectation of branches. 
Topics on the otherhand are can be made to represent Git branches as users 
expect them, see [Julia Evans poll][jvns-poll]. They also allow us to 
seamlessly take over the [tracking-branches][tb] concept.

Other use-cases they're useful for are representing a set of 
[archived commits][archived] or even a [checkout history][checkout].


#### Behavior

A Topic is a set of Changes marked with a name, which infectiously moves 
forward with each descendant you create. All Changes without a named topic are
implicitly in a anonymous topic, which gets separately tracked as soon as you 
send it in for review (so materialize it as a Git branch). A Topic may have
multiple heads. A single Change may belong to multiple Topics, which means 
that they also can get exported as overlapping Git branches. 


TODO: Example here

#### Major Use-Cases 


##### Obsoletion of `[experimental.auto-advance-bookmarks]`

Since Topics are infectious by nature, they can perfectly map to a Git branch.
This alleviates the need for the auto-advancing bookmarks. It also makes the 
use-case of the solo developer working on Git `main` easier, as you just mark
all descendants of `trunk()` belonging to the `main` topic, which then gets 
translated to the `main` branch on `jj git push`.


##### Change Archival

For the archival use-case the infectious property of a Topic isn't as 
important. So having a 
`jj topic create -r 'trunk()..@ & description("archive:")' should mark them all
revisions with a `archive:` description as archived. For this use-case the 
non-contiguous property of Topics really shine.

##### Checkout history

### Detailed Design

#### Git Interop

For all continuous Topics we can just simply export them as Git Branches. For 
Topics which contain non-contiguous parts, we should allow exporting them 
one-by-one or with a generated name, such as `<topic-name>-1` for each part.

#### Storage

We should store `Topics` as metadata on the serialized proto, without 
considering the resulting Gencode. 


```protobuf
// A simple Key-Value pair. 
message StringPair {
  string key = 1;
  string value = 2;
  // Could be extended by a protobuf.Any see the future possibilities section.
}

message Commit {
  //...
  repeated StringPair metadata = N;
}
```

while the actual code should look like this:

```rust
#[derive(ContentHash, ...)]
struct Commit {
  //...
  //
  // This avoids rewriting the Change-ID, but must be implemented.
  #[ContentHash(ignore)]
  topics: HashMap<String, String>
}
```

#### Backend implications

If Topics were stored as commit metadata, it would allow backends to drop 
them if necessary. This property can be useful to mark tests as passing
on a specific client or avoiding a field entirely in database backed backends. 

For the Git backend, we could either embed them in the message, like Arcanist 
or Gerrit do or store them as Git Notes, if necessary. 

## Alternatives considered 

### Storing Topics out-of-band 

See [Noah's prototype][prototype] for the variant of keeping them out of band.
While this works it falls short of having the metadata synced by multiple 
clients, which is something desirable. The prototype thus also avoids rewriting
the Change-ID which is a good thing, but makes them only locally available.


### Single Head Topics

While these are conceptually simpler, they wouldn't help with Git interop where
it is useful to map a single underlying Topic to multiple Git branches. This 
also worsens the `jj`-`Git` interop story.

## Future Possibilities

In the future we could attach a `google.protobuf.Any` to the Change metadata, 
which would allow specific clients, such as testrunners to directly attach test
results to a Change which could be neat. 

[archived]: https://github.com/martinvonz/jj/discussions/4180
[bm]:  ../bookmarks.md
[checkout]: https://github.com/martinvonz/jj/issues/3713
[dc-thread]: https://discord.com/channels/968932220549103686/1224085912464527502
[gerrit-topics]: https://gerrit-review.googlesource.com/Documentation/cross-repository-changes.html
[gh-discuss]: https://github.com/martinvonz/jj/discussions/2425#discussioncomment-7376935
[hg-topics]: https://www.mercurial-scm.org/doc/evolution/tutorials/topic-tutorial.html#topic-basics
[issue]: https://github.com/martinvonz/jj/discussions/2425#discussioncomment-7376935
[jvns-poll]: https://social.jvns.ca/@b0rk/111709458396281239
[prototype]: https://github.com/martinvonz/jj/pull/3613 
