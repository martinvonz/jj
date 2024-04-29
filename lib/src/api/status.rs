use crate::repo::{RepoLoaderError, StoreLoadError};
use jj_lib::op_store::OpStoreError;
use jj_lib::workspace::WorkspaceLoadError;
use tonic::Status;

impl From<StoreLoadError> for Status {
    fn from(value: StoreLoadError) -> Status {
        Status::internal(value.to_string())
    }
}

impl From<RepoLoaderError> for Status {
    fn from(value: RepoLoaderError) -> Status {
        (match value {
            RepoLoaderError::OpHeadResolution { .. } => Status::not_found,
            _ => Status::internal,
        })(value.to_string())
    }
}

impl From<OpStoreError> for Status {
    fn from(value: OpStoreError) -> Status {
        (match value {
            OpStoreError::ObjectNotFound { .. } => Status::not_found,
            _ => Status::internal,
        })(value.to_string())
    }
}

impl From<WorkspaceLoadError> for Status {
    fn from(value: WorkspaceLoadError) -> Status {
        (match value {
            WorkspaceLoadError::RepoDoesNotExist(_)
            | WorkspaceLoadError::NoWorkspaceHere(_)
            | WorkspaceLoadError::NonUnicodePath
            | WorkspaceLoadError::Path(_) => Status::invalid_argument,
            _ => Status::internal,
        })(value.to_string())
    }
}
