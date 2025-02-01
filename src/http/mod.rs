mod iri;

use anyhow::{Context, Result};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::Value;
use tokio::net::TcpListener;
use tracing::info;

use crate::activity_pub::machine::ActivityPubCommand;
use crate::activity_pub::model::{Actor, BaseObject, Collection, JsonLdValue, Object};
use crate::activity_pub::{ObjectRepo, OutboxIndex, UserIndex};
use crate::config::RuntimeConfig;
use crate::worker::raft::{LogEntryValue, RaftClientMsg, get_raft_local_client};

#[derive(Debug, Deserialize)]
struct PageParams {
    after: Option<String>,
    first: Option<u64>,
}

pub(crate) async fn serve(config: &RuntimeConfig) -> Result<()> {
    if !config.server.http.listen {
        info!(target: "http", "http API server is disabled");
        return Ok(());
    }
    let app = Router::new()
        .route("/users/{id}", get(get_actor).post(post_actor))
        .route("/users/{id}/outbox", get(get_outbox).post(post_outbox))
        .route("/users/{id}/inbox", post(post_inbox))
        .route("/users/{id}/followers", get(get_followers))
        .with_state(config.clone());
    let listener = TcpListener::bind(format!("0.0.0.0:{}", config.server.http.port)).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn get_actor(
    State(config): State<RuntimeConfig>,
    Path(uid): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let user_index = UserIndex::new(config.keyspace.clone()).map_err(ise)?;
    if let Some(object) = user_index.find_one(uid).map_err(ise)? {
        let raw_actor = Actor::try_from(object).map_err(invalid)?;
        let actor = raw_actor.enrich_with(&config.init.activity_pub);
        return Ok(Json(actor.into()));
    }
    Err(StatusCode::NOT_FOUND)
}

async fn post_actor(Path(uid): Path<String>, Json(value): Json<Value>) -> Result<(), StatusCode> {
    if value.type_is("Person") {
        let client = get_raft_local_client().map_err(ise)?;
        let command = ActivityPubCommand::UpdateUser(uid, value.into());
        ractor::call!(
            client,
            RaftClientMsg::ClientRequest,
            LogEntryValue::from(command)
        )
        .context("RPC call failed")
        .map_err(ise)?;
        return Ok(());
    }
    Err(StatusCode::BAD_REQUEST)
}

async fn get_outbox(
    State(config): State<RuntimeConfig>,
    Path(uid): Path<String>,
    Query(params): Query<PageParams>,
) -> Result<Json<Value>, StatusCode> {
    let index = OutboxIndex::new(config.keyspace.clone()).map_err(ise)?;
    if let Some(after) = params.after {
        let first = params.first.unwrap_or(10).clamp(0, 50);
        let mut items: Vec<Object> = index.find_all(uid, after, first + 1).map_err(invalid)?;
        let next = if items.len() == first as usize + 1 {
            Some(items.remove(first as usize))
        } else {
            None
        };
        let mut outbox = Collection::new().with_ordered_items(items).ordered();
        if let Some(next) = next {
            let id = next.id().expect("activity in outbox should have id");
            outbox = outbox.next(&id);
        }
        Ok(Json(outbox.to_page().into()))
    } else {
        let outbox = Collection::new()
            .first("TODO")
            .total_items(index.count(uid))
            .ordered();
        Ok(Json(outbox.into()))
    }
}

async fn post_outbox(Path(uid): Path<String>, Json(value): Json<Value>) -> Result<(), StatusCode> {
    if value.type_is("Create") || !value.is_activity() {
        let client = get_raft_local_client().map_err(ise)?;
        let command = ActivityPubCommand::C2sCreate(uid, value.into());
        ractor::call!(
            client,
            RaftClientMsg::ClientRequest,
            LogEntryValue::from(command)
        )
        .context("RPC call failed")
        .map_err(ise)?;
        return Ok(());
    }
    Err(StatusCode::BAD_REQUEST)
}

async fn post_inbox(Path(uid): Path<String>, Json(value): Json<Value>) -> Result<(), StatusCode> {
    if value.is_inbox_activity() {
        let client = get_raft_local_client().map_err(ise)?;
        let command = match value.obj_type() {
            Some("Create") => ActivityPubCommand::S2sCreate(uid, value.into()),
            Some("Delete") => ActivityPubCommand::S2sDelete(uid, value.into()),
            Some("Like") => ActivityPubCommand::S2sLike(uid, value.into()),
            Some("Dislike") => ActivityPubCommand::S2sDislike(uid, value.into()),
            Some("Follow") => ActivityPubCommand::S2sFollow(uid, value.into()),
            Some("Undo") => ActivityPubCommand::S2sUndo(uid, value.into()),
            Some("Update") => ActivityPubCommand::S2sUpdate(uid, value.into()),
            Some("Announce") => ActivityPubCommand::S2sAnnounce(uid, value.into()),
            _ => return Ok(()),
        };
        ractor::call!(
            client,
            RaftClientMsg::ClientRequest,
            LogEntryValue::from(command)
        )
        .context("RPC call failed")
        .map_err(ise)?;
        return Ok(());
    }
    return Ok(());
}

async fn get_followers(
    State(config): State<RuntimeConfig>,
    Path(uid): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let index = UserIndex::new(config.keyspace.clone()).map_err(ise)?;
    let repo = ObjectRepo::new(config.keyspace.clone()).map_err(ise)?;
    let followers = index.find_followers(uid).map_err(ise)?;
    let mut result = vec![];
    for key in followers {
        if let Some(obj) = repo.find_one(key).map_err(ise)? {
            let id = obj.id().context("object should have id").map_err(ise)?;
            result.push(Value::String(id));
        }
    }
    Ok(Json(Value::Array(result)))
}

fn ise(_error: anyhow::Error) -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
}

fn invalid(_error: anyhow::Error) -> StatusCode {
    StatusCode::UNPROCESSABLE_ENTITY
}
