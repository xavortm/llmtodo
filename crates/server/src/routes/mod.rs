pub mod activity;
pub mod actors;
pub mod analyzing;
pub mod attachments;
pub mod claims;
pub mod comments;
pub mod documents;
pub mod github;
pub mod import;
pub mod issues;
pub mod labels;
pub mod live;
pub mod projects;
pub mod prune;
pub mod questions;
pub mod relations;
pub mod reviews;
pub mod server;
pub mod sessions;
pub mod settings;
pub mod sync;
pub mod teams;
pub mod usage;
pub mod views;

use axum::Router;
use axum::extract::Request;
use axum::handler::Handler;
use axum::http::{HeaderValue, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodRouter, delete, get, patch, post};
use llmtodo_api_types::routes::{Endpoint, Method, ROUTES, ep};
use llmtodo_api_types::{ErrorCode, IssueDetail, VERSION, VERSION_HEADER};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::store;

/// The response most issue writes return.
async fn issue_detail(st: &AppState, id: &str) -> ApiResult<axum::Json<IssueDetail>> {
    let mut c = st.pool().acquire().await?;
    Ok(axum::Json(store::issue_detail::load(&mut c, id).await?))
}

/// Every handler, keyed by its endpoint type.
fn handlers() -> Vec<(Method, &'static str, MethodRouter<AppState>)> {
    vec![
        on::<ep::Health, _>(server::health),
        on::<ep::StopServer, _>(server::stop),
        on::<ep::Backup, _>(server::backup),
        on::<ep::Settings, _>(settings::get_settings),
        on::<ep::UpdateSettings, _>(settings::update_settings),
        on::<ep::Actors, _>(actors::list_actors),
        on::<ep::Me, _>(actors::me),
        on::<ep::Teams, _>(teams::list_teams),
        on::<ep::CreateTeam, _>(teams::create_team),
        on::<ep::Team, _>(teams::view_team),
        on::<ep::UpdateTeam, _>(teams::update_team),
        on::<ep::CreateStatus, _>(teams::create_status),
        on::<ep::UpdateStatus, _>(teams::update_status),
        on::<ep::DeleteStatus, _>(teams::delete_status),
        on::<ep::Labels, _>(labels::list_labels),
        on::<ep::CreateLabel, _>(labels::create_label),
        on::<ep::UpdateLabel, _>(labels::update_label),
        on::<ep::DeleteLabel, _>(labels::delete_label),
        on::<ep::Projects, _>(projects::list_projects),
        on::<ep::CreateProject, _>(projects::create_project),
        on::<ep::Project, _>(projects::view_project),
        on::<ep::UpdateProject, _>(projects::update_project),
        on::<ep::DeleteProject, _>(projects::delete_project),
        on::<ep::LinkRepo, _>(projects::link_repo),
        on::<ep::UnlinkRepo, _>(projects::unlink_repo),
        on::<ep::Milestones, _>(projects::list_milestones),
        on::<ep::CreateMilestone, _>(projects::create_milestone),
        on::<ep::UpdateMilestone, _>(projects::update_milestone),
        on::<ep::DeleteMilestone, _>(projects::delete_milestone),
        on::<ep::Documents, _>(documents::list_documents),
        on::<ep::CreateDocument, _>(documents::create_document),
        on::<ep::Document, _>(documents::view_document),
        on::<ep::UpdateDocument, _>(documents::update_document),
        on::<ep::DeleteDocument, _>(documents::delete_document),
        on::<ep::DocumentHistory, _>(documents::document_history),
        on::<ep::RestoreDocument, _>(documents::restore_document),
        on::<ep::QueryIssues, _>(issues::query),
        on::<ep::QueryIssueIds, _>(issues::query_ids),
        on::<ep::Frontier, _>(issues::frontier),
        on::<ep::CreateIssue, _>(issues::create),
        on::<ep::Issue, _>(issues::view),
        on::<ep::UpdateIssue, _>(issues::update),
        on::<ep::DeleteIssue, _>(issues::delete),
        on::<ep::RestoreIssue, _>(issues::restore),
        on::<ep::IssueHistory, _>(issues::history),
        on::<ep::Deps, _>(issues::deps),
        on::<ep::Claim, _>(claims::claim),
        on::<ep::Release, _>(claims::release),
        on::<ep::Review, _>(reviews::state),
        on::<ep::SubmitReview, _>(reviews::submit),
        on::<ep::Analyze, _>(analyzing::analyze),
        on::<ep::Questions, _>(questions::list),
        on::<ep::UndoAnswer, _>(questions::undo_answer),
        on::<ep::AddComment, _>(comments::add),
        on::<ep::UpdateComment, _>(comments::update),
        on::<ep::DeleteComment, _>(comments::delete),
        on::<ep::AddRelation, _>(relations::add_relation),
        on::<ep::RemoveRelation, _>(relations::remove_relation),
        on::<ep::Attach, _>(attachments::attach),
        on::<ep::Detach, _>(attachments::detach),
        on::<ep::Session, _>(sessions::show_session),
        on::<ep::AddSessionNote, _>(sessions::add_session_note),
        on::<ep::Usage, _>(usage::usage),
        on::<ep::Activity, _>(activity::list_activity),
        on::<ep::Ws, _>(live::ws),
        on::<ep::Views, _>(views::list_views),
        on::<ep::CreateView, _>(views::create_view),
        on::<ep::UpdateView, _>(views::update_view),
        on::<ep::DeleteView, _>(views::delete_view),
        on::<ep::SyncStatus, _>(sync::status),
        on::<ep::SyncPush, _>(sync::push),
        on::<ep::SyncPull, _>(sync::pull),
        on::<ep::GithubStatus, _>(github::status),
        on::<ep::GithubSync, _>(github::sync_now),
        on::<ep::ImportLinear, _>(import::linear),
        on::<ep::File, _>(import::file),
        on::<ep::Prune, _>(prune::prune),
    ]
}

/// Mounts `h` at the method and path of endpoint `E`.
fn on<E: Endpoint, T: 'static>(
    h: impl Handler<T, AppState>,
) -> (Method, &'static str, MethodRouter<AppState>) {
    let m = match E::METHOD {
        Method::Get => get(h),
        Method::Post => post(h),
        Method::Patch => patch(h),
        Method::Delete => delete(h),
    };
    (E::METHOD, E::PATH, m)
}

pub fn router(state: AppState) -> Router {
    let hs = handlers();
    // The route table and the handlers must agree, so the CLI coverage test
    // and `llmtodo schema` describe what the server really serves.
    for r in ROUTES {
        assert!(
            hs.iter().any(|(m, p, _)| *m == r.method && *p == r.path),
            "route table entry has no handler: {} {}",
            r.method.as_str(),
            r.path
        );
    }
    for (m, p, _) in &hs {
        assert!(
            ROUTES.iter().any(|r| r.method == *m && r.path == *p),
            "handler missing from the route table: {} {p}",
            m.as_str()
        );
    }
    let mut router: Router<AppState> = Router::new();
    let mut merged: std::collections::BTreeMap<&str, MethodRouter<AppState>> =
        std::collections::BTreeMap::new();
    for (_, path, h) in hs {
        let entry = merged.remove(path);
        merged.insert(
            path,
            match entry {
                Some(prev) => prev.merge(h),
                None => h,
            },
        );
    }
    for (path, h) in merged {
        router = router.route(path, h);
    }
    router
        .fallback(|| async {
            ApiError::new(
                ErrorCode::NotFound,
                "no such endpoint; see `llmtodo schema`",
            )
        })
        .layer(middleware::from_fn_with_state(state.clone(), record_route))
        .layer(middleware::from_fn(guard))
        .with_state(state)
}

/// Adds the request's route to `route_log` while a test records.
async fn record_route(
    axum::extract::State(st): axum::extract::State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    if let Some(log) = st.route_log.lock().unwrap().as_mut()
        && let Some(path) = req.extensions().get::<axum::extract::MatchedPath>()
    {
        log.push(format!("{} {}", req.method(), path.as_str()));
    }
    next.run(req).await
}

/// Rejects requests a web page could make, and stamps the version header.
async fn guard(req: Request, next: Next) -> Response {
    let host_ok = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(|h| {
            let name = h.rsplit_once(':').map_or(h, |(n, port)| {
                if port.bytes().all(|b| b.is_ascii_digit()) {
                    n
                } else {
                    h
                }
            });
            matches!(name, "127.0.0.1" | "localhost" | "[::1]")
        })
        .unwrap_or(false);
    if !host_ok {
        return ApiError::forbidden("requests must use Host 127.0.0.1 or localhost")
            .into_response();
    }
    if req.headers().contains_key(header::ORIGIN) {
        return ApiError::forbidden("browser requests are not allowed").into_response();
    }
    let mut res = next.run(req).await;
    res.headers_mut()
        .insert(VERSION_HEADER, HeaderValue::from_static(VERSION));
    res
}
