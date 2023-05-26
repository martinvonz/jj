#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct FileState {
    #[prost(int64, tag = "1")]
    pub mtime_millis_since_epoch: i64,
    #[prost(uint64, tag = "2")]
    pub size: u64,
    #[prost(enumeration = "FileType", tag = "3")]
    pub file_type: i32,
    /// Set only if file_type is Conflict
    #[deprecated]
    #[prost(bytes = "vec", tag = "4")]
    pub conflict_id: ::prost::alloc::vec::Vec<u8>,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SparsePatterns {
    #[prost(string, repeated, tag = "1")]
    pub prefixes: ::prost::alloc::vec::Vec<::prost::alloc::string::String>,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TreeState {
    #[prost(bytes = "vec", tag = "1")]
    pub tree_id: ::prost::alloc::vec::Vec<u8>,
    #[prost(map = "string, message", tag = "2")]
    pub file_states: ::std::collections::HashMap<
        ::prost::alloc::string::String,
        FileState,
    >,
    #[prost(message, optional, tag = "3")]
    pub sparse_patterns: ::core::option::Option<SparsePatterns>,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Checkout {
    /// The operation at which the working copy was updated.
    #[prost(bytes = "vec", tag = "2")]
    pub operation_id: ::prost::alloc::vec::Vec<u8>,
    /// An identifier for this workspace. It is used for looking up the current
    /// working-copy commit in the repo view. Currently a human-readable name.
    /// TODO: Is it better to make this a UUID and a have map that to a name in
    /// config? That way users can rename a workspace.
    #[prost(string, tag = "3")]
    pub workspace_id: ::prost::alloc::string::String,
    /// The checked-out commit, which can be viewed as a cache of the working-copy
    /// commit ID recorded in `operation_id`'s operation. No longer used.
    /// TODO: Delete this mid 2022 or so
    #[prost(bytes = "vec", tag = "1")]
    pub commit_id: ::prost::alloc::vec::Vec<u8>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum FileType {
    Normal = 0,
    Symlink = 1,
    Executable = 2,
    Conflict = 3,
    GitSubmodule = 4,
}
impl FileType {
    /// String value of the enum field names used in the ProtoBuf definition.
    ///
    /// The values are not transformed in any way and thus are considered stable
    /// (if the ProtoBuf definition does not change) and safe for programmatic use.
    pub fn as_str_name(&self) -> &'static str {
        match self {
            FileType::Normal => "Normal",
            FileType::Symlink => "Symlink",
            FileType::Executable => "Executable",
            FileType::Conflict => "Conflict",
            FileType::GitSubmodule => "GitSubmodule",
        }
    }
    /// Creates an enum from field names used in the ProtoBuf definition.
    pub fn from_str_name(value: &str) -> ::core::option::Option<Self> {
        match value {
            "Normal" => Some(Self::Normal),
            "Symlink" => Some(Self::Symlink),
            "Executable" => Some(Self::Executable),
            "Conflict" => Some(Self::Conflict),
            "GitSubmodule" => Some(Self::GitSubmodule),
            _ => None,
        }
    }
}
