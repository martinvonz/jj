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
pub struct FileStateEntry {
    #[prost(string, tag = "1")]
    pub path: ::prost::alloc::string::String,
    #[prost(message, optional, tag = "2")]
    pub state: ::core::option::Option<FileState>,
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
    pub legacy_tree_id: ::prost::alloc::vec::Vec<u8>,
    /// Alternating positive and negative terms if there's a conflict, otherwise a
    /// single (positive) value
    #[prost(bytes = "vec", repeated, tag = "5")]
    pub tree_ids: ::prost::alloc::vec::Vec<::prost::alloc::vec::Vec<u8>>,
    #[prost(message, repeated, tag = "2")]
    pub file_states: ::prost::alloc::vec::Vec<FileStateEntry>,
    #[prost(message, optional, tag = "3")]
    pub sparse_patterns: ::core::option::Option<SparsePatterns>,
    #[prost(message, optional, tag = "4")]
    pub watchman_clock: ::core::option::Option<WatchmanClock>,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct WatchmanClock {
    #[prost(oneof = "watchman_clock::WatchmanClock", tags = "1, 2")]
    pub watchman_clock: ::core::option::Option<watchman_clock::WatchmanClock>,
}
/// Nested message and enum types in `WatchmanClock`.
pub mod watchman_clock {
    #[allow(clippy::derive_partial_eq_without_eq)]
    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum WatchmanClock {
        #[prost(string, tag = "1")]
        StringClock(::prost::alloc::string::String),
        #[prost(int64, tag = "2")]
        UnixTimestamp(i64),
    }
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
