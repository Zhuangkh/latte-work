//! Persistent, independent lanes for UI reads, Agent control and slow CLI metadata.
//! Identical routing for local and SSH hosts. No requests are retried here.
use latte_work_protocol::Request;
use std::{future::Future, sync::Arc};
use tokio::sync::{Mutex, OnceCell};

#[derive(Clone, Copy)]
pub enum Lane {
    Read,
    Control,
    Metadata,
}
pub fn lane(request: &Request) -> Lane {
    match request {
        Request::OpenAgentSession { .. }
        | Request::CloseAgentSession { .. }
        | Request::Send { .. }
        | Request::Approve { .. }
        | Request::AnswerQuestion { .. }
        | Request::Cancel { .. } => Lane::Control,
        Request::AgentCommands { .. }
        | Request::AgentPermissions { .. }
        | Request::Models { .. }
        | Request::ReadFile { .. }
        | Request::SubagentTranscript { .. }
        | Request::Diff { .. }
        | Request::Changes { .. }
        | Request::ChangeDiff { .. }
        | Request::LastTurnChanges { .. }
        | Request::GitInfo { .. }
        | Request::GitCommits { .. }
        | Request::GitReview { .. }
        | Request::GitReviewDiff { .. } => Lane::Metadata,
        _ => Lane::Read,
    }
}
pub struct Channels<C> {
    pub primary: Arc<Mutex<C>>,
    control: OnceCell<Arc<Mutex<C>>>,
    metadata: OnceCell<Arc<Mutex<C>>>,
}
impl<C> Channels<C> {
    pub fn new(primary: Arc<Mutex<C>>) -> Self {
        Self {
            primary,
            control: OnceCell::new(),
            metadata: OnceCell::new(),
        }
    }
    pub async fn client<F, Fut, E>(&self, lane: Lane, connect: F) -> Result<Arc<Mutex<C>>, E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<C, E>>,
    {
        let cell = match lane {
            Lane::Read => return Ok(self.primary.clone()),
            Lane::Control => &self.control,
            Lane::Metadata => &self.metadata,
        };
        cell.get_or_try_init(|| async { Ok(Arc::new(Mutex::new(connect().await?))) })
            .await
            .cloned()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };
    #[tokio::test]
    async fn slow_control_does_not_block_history_or_metadata_and_connections_are_reused() {
        let primary = Arc::new(Mutex::new(0_u8));
        let channels = Channels::new(primary.clone());
        let connects = AtomicUsize::new(0);
        let connect = || async {
            connects.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(80)).await;
            Ok::<_, ()>(1_u8)
        };
        let (a, b, read, metadata) = tokio::join!(
            channels.client(Lane::Control, connect),
            channels.client(Lane::Control, connect),
            async {
                tokio::time::timeout(
                    Duration::from_millis(20),
                    channels.client(Lane::Read, connect),
                )
                .await
                .unwrap()
                .unwrap()
            },
            channels.client(Lane::Metadata, || async { Ok::<_, ()>(2) })
        );
        let a = a.unwrap();
        assert!(Arc::ptr_eq(&a, &b.unwrap()));
        assert!(Arc::ptr_eq(&primary, &read));
        assert!(!Arc::ptr_eq(&a, &metadata.unwrap()));
        assert_eq!(connects.load(Ordering::SeqCst), 1);
        let held = a.lock().await;
        assert!(
            tokio::time::timeout(Duration::from_millis(20), read.lock())
                .await
                .is_ok()
        );
        drop(held);
        assert!(Arc::ptr_eq(
            &a,
            &channels.client(Lane::Control, connect).await.unwrap()
        ));
        assert_eq!(connects.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn failed_initialization_is_not_cached_and_new_primary_gets_new_lanes() {
        let primary = Arc::new(Mutex::new(0));
        let channels = Channels::new(primary);
        assert!(
            channels
                .client(Lane::Control, || async { Err::<u8, _>(()) })
                .await
                .is_err()
        );
        let old = channels
            .client(Lane::Control, || async { Ok::<_, ()>(1) })
            .await
            .unwrap();
        let replacement = Channels::new(Arc::new(Mutex::new(2)));
        let new = replacement
            .client(Lane::Control, || async { Ok::<_, ()>(3) })
            .await
            .unwrap();
        assert!(!Arc::ptr_eq(&old, &new));
    }
}
