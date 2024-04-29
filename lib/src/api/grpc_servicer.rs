use crate::api::servicer::Servicer;
use jj_api::rpc::{ListWorkspacesRequest, ListWorkspacesResponse};
use jj_api::server::JjService;
use tonic::{Request, Response, Status};

pub struct GrpcServicer {
    servicer: Servicer,
}

impl GrpcServicer {
    pub fn new(servicer: Servicer) -> Self {
        Self { servicer }
    }
}

#[tonic::async_trait]
impl JjService for GrpcServicer {
    // TODO: this should be boilerplate. Maybe turn it into macros.
    // eg. rpc!(list_workspaces, ListWorkspacesRequest, ListWorkspacesResponse)
    async fn list_workspaces(
        &self,
        request: Request<ListWorkspacesRequest>,
    ) -> Result<Response<ListWorkspacesResponse>, Status> {
        self.servicer
            .list_workspaces(request.get_ref())
            .map(Response::new)
    }
}
