mod generated;

// Because we declare all our .proto files under the package "jj_api", inside the crate jj_api, we
// need to un-nest them so that they don't appear as jj_api::jj_api::foo.
pub use crate::generated::jj_api::*;

mod services {
    include!("generated/jj_api.services.rs");
}

pub use services::jj_service_client as client;
pub use services::jj_service_server as server;

mod to_proto;
pub use to_proto::ToProto;

pub mod from_proto;
