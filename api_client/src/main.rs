use jj_api::client::JjServiceClient;
use jj_api::rpc::ListWorkspacesRequest;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = JjServiceClient::connect("http://[::1]:8888").await?;

    let request = tonic::Request::new(ListWorkspacesRequest::default());

    let response = client.list_workspaces(request).await?;

    println!("RESPONSE={:?}", response);

    Ok(())
}
