use clap::Subcommand;
use std::io::Write;

use crate::api::workspaces::WorkspaceBody;
use crate::db::repos::traits::Workspace;

use super::client::ApiClient;
use super::util;

#[derive(Subcommand, Debug)]
pub enum WorkspacesCmd {
    /// List all workspaces (most commands need a workspace id from here)
    List,
    /// Create a workspace
    #[command(
        after_help = "EXAMPLE:\n  pulp workspaces create \"acme launch\" --description \"watch the acme.dev launch\""
    )]
    Create {
        /// Workspace name
        name: String,
        #[arg(long)]
        description: Option<String>,
    },
    /// Rename / re-describe a workspace
    Update {
        /// Workspace id
        id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        description: Option<String>,
    },
    /// Delete a workspace (cascades to its monitors and notifications)
    Delete {
        /// Workspace id
        id: String,
    },
}

pub async fn run(
    cmd: WorkspacesCmd,
    client: &ApiClient,
    json: bool,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    match cmd {
        WorkspacesCmd::List => {
            let workspaces: Vec<Workspace> = client.get("/api/workspaces").await?;
            if json {
                util::print_json(out, &workspaces)?;
            } else if workspaces.is_empty() {
                writeln!(
                    out,
                    "no workspaces — create one with `pulp workspaces create <name>`"
                )?;
            } else {
                writeln!(out, "{:<26}  {:<20}  DESCRIPTION", "ID", "NAME")?;
                for w in &workspaces {
                    writeln!(
                        out,
                        "{:<26}  {:<20}  {}",
                        w.id,
                        w.name,
                        w.description.as_deref().unwrap_or("-")
                    )?;
                }
            }
        }
        WorkspacesCmd::Create { name, description } => {
            let body = WorkspaceBody { name, description };
            let w: Workspace = client.post("/api/workspaces", &body).await?;
            if json {
                util::print_json(out, &w)?;
            } else {
                writeln!(out, "created workspace '{}' (id: {})", w.name, w.id)?;
            }
        }
        WorkspacesCmd::Update {
            id,
            name,
            description,
        } => {
            let body = WorkspaceBody { name, description };
            let w: Workspace = client
                .put(&format!("/api/workspaces/{}", id), &body)
                .await?;
            if json {
                util::print_json(out, &w)?;
            } else {
                writeln!(out, "updated workspace '{}' (id: {})", w.name, w.id)?;
            }
        }
        WorkspacesCmd::Delete { id } => {
            client.delete(&format!("/api/workspaces/{}", id)).await?;
            writeln!(out, "deleted workspace {}", id)?;
        }
    }
    Ok(())
}
