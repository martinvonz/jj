use crate::api::from_proto;
use crate::object_id::ObjectId;
use crate::repo::ReadonlyRepo;
use crate::settings::UserSettings;
use config::Config;
use itertools::Itertools;
use jj_api::objects::{Change as ChangeProto, Workspace as WorkspaceProto};
use jj_api::rpc::{ListWorkspacesRequest, ListWorkspacesResponse};
use jj_lib::op_store::OperationId;
use jj_lib::operation::Operation;
use jj_lib::repo::RepoLoader;
use jj_lib::workspace::WorkspaceLoader;

use std::sync::Arc;
use tonic::Status;

/// The servicer handles all requests going to jj-lib. Eventually, ideally, jj-cli
/// will interact with jj-lib purely through this class.
pub struct Servicer {
    default_workspace_loader: Option<WorkspaceLoader>,
    user_settings: UserSettings,
}

impl Servicer {
    pub fn new(default_workspace_loader: Option<WorkspaceLoader>) -> Self {
        Self {
            default_workspace_loader,
            user_settings: UserSettings::from_config(Config::default()),
        }
    }

    fn workspace_loader(
        &self,
        opts: &Option<jj_api::objects::RepoOptions>,
    ) -> Result<WorkspaceLoader, Status> {
        opts.as_ref()
            .map(|opts| from_proto::path(&opts.repo_path))
            .flatten()
            .map(WorkspaceLoader::init)
            .transpose()?
            .or(self.default_workspace_loader.clone())
            .ok_or_else(|| {
                Status::invalid_argument(
                    "No default workspace loader, and no repository.repo_path provided",
                )
            })
    }

    fn repo(
        &self,
        opts: &Option<jj_api::objects::RepoOptions>,
    ) -> Result<Arc<ReadonlyRepo>, Status> {
        let workspace_loader = self.workspace_loader(opts)?;

        let at_operation: Option<OperationId> = opts
            .as_ref()
            .map(|opts| from_proto::operation_id(&opts.at_operation))
            .transpose()?
            .flatten();

        let repo_loader = RepoLoader::init(
            &self.user_settings,
            &workspace_loader.repo_path(),
            &Default::default(),
        )?;

        Ok(match at_operation {
            None => repo_loader.load_at_head(&self.user_settings),
            Some(at_operation) => {
                let op = repo_loader.op_store().read_operation(&at_operation)?;
                repo_loader.load_at(&Operation::new(
                    repo_loader.op_store().clone(),
                    at_operation,
                    op,
                ))
            }
        }?)
    }

    pub fn list_workspaces(
        &self,
        request: &ListWorkspacesRequest,
    ) -> Result<ListWorkspacesResponse, Status> {
        let repo = self.repo(&request.repo)?;
        Ok(ListWorkspacesResponse {
            workspace: repo
                .view()
                .wc_commit_ids()
                .iter()
                .sorted()
                .map(|(workspace_id, commit_id)| WorkspaceProto {
                    workspace_id: workspace_id.as_str().to_string(),
                    change: Some(ChangeProto {
                        commit_id: commit_id.hex(),
                        ..Default::default()
                    }),
                })
                .collect::<Vec<WorkspaceProto>>(),
        })
    }
}
