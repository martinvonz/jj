use std::path::{Path, PathBuf};

pub trait ToProto<T> {
    fn to_proto(&self) -> T;
}

// For some reason I was getting errors with `for AsRef<Path>`.
impl ToProto<String> for Path {
    fn to_proto(&self) -> String {
        self.to_string_lossy().to_string()
    }
}

impl ToProto<String> for PathBuf {
    fn to_proto(&self) -> String {
        self.to_string_lossy().to_string()
    }
}
