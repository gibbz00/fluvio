//!
//! # Topic Controller
//!
//! Reconcile Topics

use fluvio_stream_model::core::MetadataItem;
use fluvio_stream_model::store::ChangeListener;
use fluvio_stream_model::store::k8::K8MetaItem;
use tracing::debug;
use tracing::instrument;

use fluvio_future::task::spawn;

use crate::core::SharedContext;
use crate::stores::topic::TopicSpec;
use crate::stores::partition::PartitionSpec;
use crate::stores::StoreContext;

use super::reducer::TopicReducer;

#[derive(Debug)]
pub struct TopicController<C: MetadataItem = K8MetaItem> {
    topics: StoreContext<TopicSpec, C>,
    partitions: StoreContext<PartitionSpec, C>,
    reducer: TopicReducer<C>,
}

impl<C> TopicController<C>
where
    C: MetadataItem + 'static,
    C::UId: Send + Sync,
{
    /// streaming coordinator controller constructor
    pub fn start(ctx: SharedContext<C>) {
        let topics = ctx.topics().clone();
        let partitions = ctx.partitions().clone();

        let controller = Self {
            reducer: TopicReducer::new(
                topics.store().clone(),
                ctx.spus().store().clone(),
                partitions.store().clone(),
            ),
            topics,
            partitions,
        };

        spawn(controller.dispatch_loop());
    }
}

impl<C: MetadataItem> TopicController<C> {
    #[instrument(name = "TopicController", skip(self))]
    async fn dispatch_loop(mut self) {
        use std::time::Duration;

        use tokio::select;
        use fluvio_future::timer::sleep;

        debug!("starting dispatch loop");

        let mut listener = self.topics.change_listener();

        loop {
            self.sync_topics(&mut listener).await;

            select! {

                // just in case
                _ = sleep(Duration::from_secs(60)) => {
                    debug!("timer expired");
                },
                _ = listener.listen() => {
                    debug!("detected topic changes");

                }
            }
        }
    }

    #[instrument(skip(self, listener))]
    async fn sync_topics(&mut self, listener: &mut ChangeListener<TopicSpec, C>) {
        if !listener.has_change() {
            debug!("no change");
            return;
        }

        let changes = listener.sync_changes().await;

        if changes.is_empty() {
            debug!("no topic changes");
            return;
        }

        let (updates, _) = changes.parts();

        let actions = self.reducer.process_requests(updates).await;

        if actions.topics.is_empty() && actions.partitions.is_empty() {
            debug!("no actions needed");
        } else {
            debug!(
                "sending topic actions: {}, partition actions: {}",
                actions.topics.len(),
                actions.partitions.len()
            );
            for action in actions.topics.into_iter() {
                self.topics.send_action(action).await;
            }

            for action in actions.partitions.into_iter() {
                self.partitions.send_action(action).await;
            }
        }
    }
}
