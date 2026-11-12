//! Documents.

use std::path::PathBuf;

use clap::Subcommand;
use llmtodo_api_types::*;

use crate::{App, CliError, CliResult, read_text};

#[derive(Subcommand)]
pub enum DocCmd {
    /// List documents, or search their titles and bodies.
    List {
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        trashed: bool,
        /// Keep only documents that match this text, best match first.
        #[arg(long)]
        search: Option<String>,
    },
    /// Read a document by ID or `project/slug`.
    View {
        doc: String,
    },
    Create {
        title: String,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        slug: Option<String>,
        #[arg(long, short = 'b')]
        body: Option<String>,
        #[arg(long)]
        body_file: Option<PathBuf>,
    },
    /// Change a title, replace the body, or patch one section.
    Update {
        doc: String,
        #[arg(long)]
        title: Option<String>,
        /// Replace the whole body. Needs --base-version or --force.
        #[arg(long)]
        body: Option<String>,
        #[arg(long)]
        body_file: Option<PathBuf>,
        #[arg(long)]
        base_version: Option<i64>,
        #[arg(long)]
        force: bool,
        /// Replace one section's body, chosen by heading or `+++` title.
        #[arg(long)]
        section: Option<String>,
        #[arg(long)]
        section_body: Option<String>,
        #[arg(long)]
        section_body_file: Option<PathBuf>,
        #[arg(long, requires = "with")]
        replace: Option<String>,
        /// The new text for --replace.
        #[arg(long, requires = "replace")]
        with: Option<String>,
    },
    Delete {
        doc: String,
    },
    History {
        doc: String,
        /// Include the full text of every revision.
        #[arg(long)]
        bodies: bool,
    },
    Restore {
        doc: String,
        #[arg(long)]
        revision: Option<i64>,
    },
}

pub async fn run(app: &App, cmd: DocCmd) -> CliResult {
    let c = app.client().await?;
    match cmd {
        DocCmd::List {
            project,
            trashed,
            search,
        } => app.out.ser(
            &c.documents(app.project(project).as_deref(), trashed, search.as_deref())
                .await?,
        ),
        DocCmd::View { doc } => {
            let d = c.document(&doc).await?;
            if app.out.json {
                app.out.ser(&d);
            } else {
                println!(
                    "{}/{}  {}  ·  version {}\n\n{}",
                    d.project,
                    d.slug,
                    d.title,
                    d.version,
                    d.body.unwrap_or_default().trim_end()
                );
            }
        }
        DocCmd::Create {
            title,
            project,
            slug,
            body,
            body_file,
        } => {
            let project = app.project(project).ok_or_else(|| {
                CliError::Usage("pass --project or set project in .llmtodo.toml".into())
            })?;
            let body = read_text(body, body_file)?.unwrap_or_default();
            app.out.ser(
                &c.create_document(&CreateDocument {
                    project,
                    title,
                    slug,
                    body,
                })
                .await?,
            )
        }
        DocCmd::Update {
            doc,
            title,
            body,
            body_file,
            base_version,
            force,
            section,
            section_body,
            section_body_file,
            replace,
            with,
        } => {
            let mut patches = Vec::new();
            if let Some(section) = section {
                let body = read_text(section_body, section_body_file)?.ok_or_else(|| {
                    CliError::Usage("--section needs --section-body or --section-body-file".into())
                })?;
                patches.push(Patch::ReplaceSection { section, body });
            }
            if let Some(old) = replace {
                patches.push(Patch::ReplaceExact {
                    old,
                    new: with.unwrap_or_default(),
                });
            }
            let u = UpdateDocument {
                title,
                body: BodyChange {
                    body: read_text(body, body_file)?,
                    base_version,
                    force,
                    patches,
                },
            };
            if u.title.is_none() && u.body.body.is_none() && u.body.patches.is_empty() {
                return Err(CliError::Usage(
                    "nothing to change: pass --title, a body or a patch".into(),
                ));
            }
            app.out.ser(&c.update_document(&doc, &u).await?)
        }
        DocCmd::Delete { doc } => {
            app.confirm(&format!("Move document {doc} to the trash?"))?;
            app.out.ser(&c.delete_document(&doc).await?)
        }
        DocCmd::History { doc, bodies } => {
            let q = HistoryQuery { bodies };
            app.out.ser(&c.document_history(&doc, &q).await?)
        }
        DocCmd::Restore { doc, revision } => app.out.ser(
            &c.restore_document(&doc, &RestoreRequest { revision })
                .await?,
        ),
    }
    Ok(())
}
