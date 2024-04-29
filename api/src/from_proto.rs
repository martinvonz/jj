use std::path::{Path, PathBuf};
use tonic::Status;

pub fn option_str(value: &str) -> Option<&str> {
    if value == "" {
        None
    } else {
        Some(&value)
    }
}

pub fn path(value: &str) -> Option<&Path> {
    option_str(value).map(Path::new)
}

pub fn pathbuf(value: &str) -> Option<PathBuf> {
    path(value).map(PathBuf::from)
}

pub fn hex(value: &str) -> Result<Option<Vec<u8>>, Status> {
    option_str(value)
        .map(hex::decode)
        .transpose()
        .map_err(|err| Status::invalid_argument(err.to_string()))
}
