use ractor::{Actor, ActorProcessingErr, ActorRef, SupervisionEvent};
use ractor_cluster::RactorMessage;
use tracing::info;

use crate::activity_pub::delivery::{DeliveryWorker, DeliveryWorkerInit, DeliveryWorkerMsg};
use crate::activity_pub::machine::{ActivityPubMachine, ActivityPubMachineInit};
use crate::config::{RuntimeConfig, ServerConfig};
use crate::feed_slurp::{FeedSlurpMsg, FeedSlurpWorker, FeedSlurpWorkerInit};
use crate::flags::Serve;
use crate::raft::{RaftServer, RaftServerMsg, StateMachineMsg};

use super::cluster::{ClusterMaint, ClusterMaintMsg};
use super::manhole::{Manhole, ManholeMsg};

pub(crate) struct Supervisor;

#[derive(RactorMessage)]
pub(crate) enum SupervisorMsg {}

pub(crate) struct SupervisorState {
    server: ServerConfig,
    config: RuntimeConfig,
    #[allow(unused)]
    myself: ActorRef<SupervisorMsg>,
}

impl Actor for Supervisor {
    type Msg = SupervisorMsg;
    type State = SupervisorState;
    type Arguments = (Serve, RuntimeConfig);

    async fn pre_start(
        &self,
        myself: ActorRef<Self::Msg>,
        args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        let config = args.1.clone();
        let server = config.server.clone();

        Actor::spawn_linked(
            Some("cluster_maint".into()),
            ClusterMaint,
            (server.clone(), config.clone()),
            myself.get_cell(),
        )
        .await?;

        Actor::spawn_linked(
            Some("manhole".into()),
            Manhole,
            (server.clone(), config.clone()),
            myself.get_cell(),
        )
        .await?;

        Actor::spawn_linked(None, RaftServer, config.clone(), myself.get_cell()).await?;

        Actor::spawn_linked(
            Some("state_machine".into()),
            ActivityPubMachine,
            ActivityPubMachineInit {
                apub: config.init.activity_pub.clone(),
                keyspace: config.keyspace.clone(),
            },
            myself.get_cell(),
        )
        .await?;

        Actor::spawn_linked(
            None,
            DeliveryWorker,
            DeliveryWorkerInit {
                config: config.clone(),
            },
            myself.get_cell(),
        )
        .await?;

        Actor::spawn_linked(
            Some("feed_slurp".to_string()),
            FeedSlurpWorker,
            FeedSlurpWorkerInit {
                apub: config.init.activity_pub.clone(),
                config: config.init.feed_slurp.clone(),
            },
            myself.get_cell(),
        )
        .await?;

        Ok(SupervisorState {
            server,
            config,
            myself,
        })
    }

    async fn post_start(
        &self,
        _myself: ActorRef<Self::Msg>,
        _state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        info!(target: "lifecycle", "started");
        Ok(())
    }

    async fn handle_supervisor_evt(
        &self,
        myself: ActorRef<Self::Msg>,
        message: SupervisionEvent,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        use SupervisionEvent::*;

        match message {
            ActorStarted(_) => {}
            ActorTerminated(_, _, _) => {}
            ActorFailed(actor_cell, error) => {
                if matches!(
                    actor_cell.is_message_type_of::<ClusterMaintMsg>(),
                    Some(true)
                ) {
                    info!(target: "supervision", error, "cluster_maint crashed, restarting...");
                    Actor::spawn_linked(
                        Some("cluster_maint".into()),
                        ClusterMaint,
                        (state.server.clone(), state.config.clone()),
                        myself.get_cell(),
                    )
                    .await?;
                }
                if matches!(actor_cell.is_message_type_of::<ManholeMsg>(), Some(true)) {
                    info!(target: "supervision", error, "manhole crashed, restarting...");
                    Actor::spawn_linked(
                        Some("manhole".into()),
                        Manhole,
                        (state.server.clone(), state.config.clone()),
                        myself.get_cell(),
                    )
                    .await?;
                }
                if matches!(actor_cell.is_message_type_of::<RaftServerMsg>(), Some(true)) {
                    info!(target: "supervision", error, "raft server crashed, restarting...");
                    Actor::spawn_linked(None, RaftServer, state.config.clone(), myself.get_cell())
                        .await?;
                }
                if matches!(
                    actor_cell.is_message_type_of::<StateMachineMsg>(),
                    Some(true)
                ) {
                    info!(target: "supervision", error, "state machine crashed, restarting...");
                    Actor::spawn_linked(
                        Some("state_machine".into()),
                        ActivityPubMachine,
                        ActivityPubMachineInit {
                            apub: state.config.init.activity_pub.clone(),
                            keyspace: state.config.keyspace.clone(),
                        },
                        myself.get_cell(),
                    )
                    .await?;
                }
                if matches!(
                    actor_cell.is_message_type_of::<DeliveryWorkerMsg>(),
                    Some(true)
                ) {
                    info!(target: "supervision", error, "delivery worker crashed, restarting...");
                    Actor::spawn_linked(
                        None,
                        DeliveryWorker,
                        DeliveryWorkerInit {
                            config: state.config.clone(),
                        },
                        myself.get_cell(),
                    )
                    .await?;
                }
                if matches!(actor_cell.is_message_type_of::<FeedSlurpMsg>(), Some(true)) {
                    info!(target: "supervision", error, "feed slurp worker crashed, restarting...");
                    Actor::spawn_linked(
                        Some("feed_slurp".to_string()),
                        FeedSlurpWorker,
                        FeedSlurpWorkerInit {
                            apub: state.config.init.activity_pub.clone(),
                            config: state.config.init.feed_slurp.clone(),
                        },
                        myself.get_cell(),
                    )
                    .await?;
                }
            }
            ProcessGroupChanged(_) => {}
            PidLifecycleEvent(_) => {}
        }

        Ok(())
    }
}
