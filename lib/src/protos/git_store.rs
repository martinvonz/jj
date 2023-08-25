#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Commit {
    #[prost(bytes = "vec", repeated, tag = "2")]
    pub predecessors: ::prost::alloc::vec::Vec<::prost::alloc::vec::Vec<u8>>,
    #[prost(bytes = "vec", tag = "4")]
    pub change_id: ::prost::alloc::vec::Vec<u8>,
    /// Alternating positive and negative terms. Set only for conflicts.
    /// Resolved trees are stored in the git commit
    #[prost(bytes = "vec", repeated, tag = "1")]
    pub root_tree: ::prost::alloc::vec::Vec<::prost::alloc::vec::Vec<u8>>,
    /// TODO(#1624): delete when we assume that all commits use this format
    #[prost(bool, tag = "10")]
    pub uses_tree_conflict_format: bool,
    #[deprecated]
    #[prost(bool, tag = "8")]
    pub is_open: bool,
    #[deprecated]
    #[prost(bool, tag = "9")]
    pub is_pruned: bool,
}
