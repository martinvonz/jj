# Roadmap

This documents some of the goals we have. Many of them are quite independent.

> **Note:** Most people contributing to Jujutsu do so in their spare time, which
>  means that we cannot attach any target dates to any of the goals below.

## Support for copies and renames

We want to support copy tracing in a way that leaves it up to the commit backend
to either record or detect copies. That should let us work with existing Git
repos (Git does not record copies, it detects them on the fly) as well as with
very large repos where detection would be too slow. See
[design doc][copy-design-doc].

## Forge integrations

We would like to make it easier to work with various popular forges by providing
something like `jj github submit`, `jj gitlab submit`, and `jj gerrit send`. For
popular forges, we might include that support by default in the standard `jj`
binary.

## Submodule support

Git submodules are used frequently enough in large Git repos that we will
probably need to [support them][submodules]. There are still big open
questions around UX.

## Better Rust API for UIs

UIs like [gg] currently have to duplicate quite a bit of logic from `jj-cli`. We
need to make this code not specific to the CLI (e.g. return status objects
instead of printing messages) and move it into `jj-lib`.

## RPC API

One problem with writing tools using the Rust API is that they will only work
with the backends they were compiled with. For example, a regular [gg] build
will not work on Google repos because it doesn't have the backends necessary to
load them. We want to provide an RPC API for tools that want to work with an
unknown build of `jj` by having the tool run something like `jj api` to give it
an address to talk to.

In addition to helping with the problem of unknown backends, having an RPC API
should make it easier for tools like VS Code that are not written in Rust. The
RPC API will probably be at a higher abstraction level than the Rust API.

See [design doc][api-design-doc].

## Open-source cloud-based repos (server and daemon process)

Google has an internal Jujutsu server backed by a database. This server allows
commits and repos (operation logs) to be stored in the cloud (i.e. the database).
Working copies can still be stored locally.

In order to reduce latency, there is a local daemon process that caches reads
and writes. It also prefetches of objects it thinks the client might as for
next. In also helps with write latency by optimistically answering write
requests (it therefore needs to know the server's hashing scheme so it can
return the right IDs).

We (the project, not necessarily Google) want to provide a similar experience
for all users. We would therefore like to create a similar server and daemon.
The daemon might be the same process as for the RPC API mentioned above.

## Virtual file system (VFS)

For very large projects and/or large files, it can be expensive to update the
working copy. We want to provide a VFS to help with that. Updating the working
copy to another commit can then be done simply by telling the VFS to use the
other commit as base, without needing to download any large files in the target
commit until the user asks for them via the file system. A VFS can also make it
cheap to snapshot the working copy by keeping track of all changes compared to
the base commit.

Having a VFS can also be very benefial for [`jj run`][jj-run], since we can then
cheaply create temporary working copies for the commands to run in.

## Better support for large files

We have talked about somehow using content-defined chunking (CDC) to reduce
storage and transfer costs for large files. Maybe we will store files in our
future cloud-based server using the same model as [XetHub][xet-storage].


[api-design-doc]: https://docs.google.com/document/d/1rOKvutee5TVYpFhh_UDNZDxfUKyrJ8rjCNpFaNHOHwU/edit?usp=sharing&resourcekey=0-922ApyoAjuXN_uTKqmCqjg
[copy-design-doc]: design/copy-tracking.md
[gg]: https://github.com/gulbanana/gg
[jj-run]: https://github.com/martinvonz/jj/issues/1869
[submodules]: https://github.com/martinvonz/jj/issues/494
[xet-storage]: https://xethub.com/assets/docs/concepts/xet-storage
