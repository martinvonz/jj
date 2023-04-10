#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct GitRef {
    #[prost(string, tag = "1")]
    pub name: ::prost::alloc::string::String,
    #[prost(bytes = "vec", tag = "2")]
    pub commit_id: ::prost::alloc::vec::Vec<u8>,
}
/// This is the view of the last seen state of refs in the backing Git
/// repository. Unlike the refs in jj's "View", these are not affected by `jj
/// undo`. They also do not support conflicted states. Note that this implies
/// that this data does not support concurrent changes and should be modified
/// under lock.
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct GitRefView {
    /// TODO: We may eventually also track the HEAD (or HEADs if we support
    /// multiple worktrees) here as well. For now, we do not allow it to be
    /// conflicted.
    #[prost(message, repeated, tag = "1")]
    pub refs: ::prost::alloc::vec::Vec<GitRef>,
}
