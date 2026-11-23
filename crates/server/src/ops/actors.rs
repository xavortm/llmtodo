//! Actors.

use crate::store::backend::Conn;
use llmtodo_api_types::*;

use crate::error::ApiResult;
use crate::store::actors;

/// All actors: the owner first, then by kind and name.
pub async fn list(c: &mut Conn) -> ApiResult<ActorList> {
    let mut actors: Vec<Actor> = actors::all(c).await?.into_values().collect();
    actors.sort_by(|a, b| {
        (!a.is_owner, a.kind.as_str(), a.name.to_lowercase()).cmp(&(
            !b.is_owner,
            b.kind.as_str(),
            b.name.to_lowercase(),
        ))
    });
    Ok(ActorList { actors })
}
