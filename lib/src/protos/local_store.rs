#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TreeValue {
    #[prost(oneof = "tree_value::Value", tags = "2, 3, 4, 5")]
    pub value: ::core::option::Option<tree_value::Value>,
}
/// Nested message and enum types in `TreeValue`.
pub mod tree_value {
    #[allow(clippy::derive_partial_eq_without_eq)]
    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct File {
        #[prost(bytes = "vec", tag = "1")]
        pub id: ::prost::alloc::vec::Vec<u8>,
        #[prost(bool, tag = "2")]
        pub executable: bool,
    }
    #[allow(clippy::derive_partial_eq_without_eq)]
    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum Value {
        #[prost(message, tag = "2")]
        File(File),
        #[prost(bytes, tag = "3")]
        SymlinkId(::prost::alloc::vec::Vec<u8>),
        #[prost(bytes, tag = "4")]
        TreeId(::prost::alloc::vec::Vec<u8>),
        #[prost(bytes, tag = "5")]
        ConflictId(::prost::alloc::vec::Vec<u8>),
    }
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Tree {
    #[prost(message, repeated, tag = "1")]
    pub entries: ::prost::alloc::vec::Vec<tree::Entry>,
}
/// Nested message and enum types in `Tree`.
pub mod tree {
    #[allow(clippy::derive_partial_eq_without_eq)]
    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct Entry {
        #[prost(string, tag = "1")]
        pub name: ::prost::alloc::string::String,
        #[prost(message, optional, tag = "2")]
        pub value: ::core::option::Option<super::TreeValue>,
    }
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Commit {
    #[prost(bytes = "vec", repeated, tag = "1")]
    pub parents: ::prost::alloc::vec::Vec<::prost::alloc::vec::Vec<u8>>,
    #[prost(bytes = "vec", repeated, tag = "2")]
    pub predecessors: ::prost::alloc::vec::Vec<::prost::alloc::vec::Vec<u8>>,
    /// Alternating positive and negative terms
    #[prost(bytes = "vec", repeated, tag = "3")]
    pub root_tree: ::prost::alloc::vec::Vec<::prost::alloc::vec::Vec<u8>>,
    /// TODO(#1624): delete when all code paths can handle this format
    #[prost(bool, tag = "8")]
    pub uses_tree_conflict_format: bool,
    #[prost(bytes = "vec", tag = "4")]
    pub change_id: ::prost::alloc::vec::Vec<u8>,
    #[prost(string, tag = "5")]
    pub description: ::prost::alloc::string::String,
    #[prost(message, optional, tag = "6")]
    pub author: ::core::option::Option<commit::Signature>,
    #[prost(message, optional, tag = "7")]
    pub committer: ::core::option::Option<commit::Signature>,
    #[prost(bytes = "vec", optional, tag = "9")]
    pub secure_sig: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
}
/// Nested message and enum types in `Commit`.
pub mod commit {
    #[allow(clippy::derive_partial_eq_without_eq)]
    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct Timestamp {
        #[prost(int64, tag = "1")]
        pub millis_since_epoch: i64,
        #[prost(int32, tag = "2")]
        pub tz_offset: i32,
    }
    #[allow(clippy::derive_partial_eq_without_eq)]
    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct Signature {
        #[prost(string, tag = "1")]
        pub name: ::prost::alloc::string::String,
        #[prost(string, tag = "2")]
        pub email: ::prost::alloc::string::String,
        #[prost(message, optional, tag = "3")]
        pub timestamp: ::core::option::Option<Timestamp>,
    }
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Conflict {
    #[prost(message, repeated, tag = "1")]
    pub removes: ::prost::alloc::vec::Vec<conflict::Term>,
    #[prost(message, repeated, tag = "2")]
    pub adds: ::prost::alloc::vec::Vec<conflict::Term>,
}
/// Nested message and enum types in `Conflict`.
pub mod conflict {
    #[allow(clippy::derive_partial_eq_without_eq)]
    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct Term {
        #[prost(message, optional, tag = "1")]
        pub content: ::core::option::Option<super::TreeValue>,
    }
}
