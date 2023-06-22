#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TreeConflict {
    #[prost(bytes = "vec", repeated, tag = "1")]
    pub removes: ::prost::alloc::vec::Vec<::prost::alloc::vec::Vec<u8>>,
    #[prost(bytes = "vec", repeated, tag = "2")]
    pub adds: ::prost::alloc::vec::Vec<::prost::alloc::vec::Vec<u8>>,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Commit {
    #[prost(bytes = "vec", repeated, tag = "2")]
    pub predecessors: ::prost::alloc::vec::Vec<::prost::alloc::vec::Vec<u8>>,
    #[prost(bytes = "vec", tag = "4")]
    pub change_id: ::prost::alloc::vec::Vec<u8>,
    /// TODO(#1624): delete when we assume that all commits use this format
    #[prost(bool, tag = "10")]
    pub uses_tree_conflict_format: bool,
    #[deprecated]
    #[prost(bool, tag = "8")]
    pub is_open: bool,
    #[deprecated]
    #[prost(bool, tag = "9")]
    pub is_pruned: bool,
    #[prost(oneof = "commit::RootTree", tags = "3, 1")]
    pub root_tree: ::core::option::Option<commit::RootTree>,
}
/// Nested message and enum types in `Commit`.
pub mod commit {
    #[allow(clippy::derive_partial_eq_without_eq)]
    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum RootTree {
        #[prost(bytes, tag = "3")]
        Resolved(::prost::alloc::vec::Vec<u8>),
        #[prost(message, tag = "1")]
        Conflict(super::TreeConflict),
    }
}
