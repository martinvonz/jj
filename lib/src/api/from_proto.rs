use crate::op_store::OperationId;
use jj_api::from_proto;
use tonic::Status;

pub(crate) use jj_api::from_proto::*;

pub(crate) fn operation_id(value: &str) -> Result<Option<OperationId>, Status> {
    Ok(from_proto::hex(value)?.map(|bytes| OperationId::from_bytes(&bytes)))
}
