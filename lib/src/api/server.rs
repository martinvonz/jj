use crate::api::grpc_servicer::GrpcServicer;
use crate::api::servicer::Servicer;
use jj_api::server::JjServiceServer;
use tonic::transport::Server;

pub enum StartupOptions {
    Grpc(GrpcOptions),
}

pub struct GrpcOptions {
    pub port: u16,
    pub web: bool,
}

#[tokio::main(flavor = "current_thread")]
pub async fn start_api(
    options: StartupOptions,
    servicer: Servicer,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match options {
        StartupOptions::Grpc(options) => start_grpc(options, servicer),
    }
    .await
}

pub async fn start_grpc(
    options: GrpcOptions,
    servicer: Servicer,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr = format!("[::1]:{}", options.port).parse()?;

    let server = JjServiceServer::new(GrpcServicer::new(servicer));

    let mut builder = Server::builder()
        // The gRPC server is inherently async, but we want it to be synchronous.
        .concurrency_limit_per_connection(1);
    if options.web {
        // GrpcWeb is over http1 so we must enable it.
        builder
            .accept_http1(true)
            .add_service(tonic_web::enable(server))
    } else {
        builder.add_service(server)
    }
    .serve(addr)
    .await?;
    Ok(())
}
