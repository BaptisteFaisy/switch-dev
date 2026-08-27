use serde::Serialize;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tokio::sync::broadcast;

const RUNTIME_SYNC_BUFFER: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum RuntimeSyncTopic {
    ActiveChatTurns,
    ActiveTerminals,
    AutonomousAgents,
    AccountCompletions,
    PrivateMessages,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeSyncEvent {
    pub topic: RuntimeSyncTopic,
    pub revision: u64,
    user_ids: Option<Vec<String>>,
}

impl RuntimeSyncEvent {
    pub fn is_visible_to(&self, user_id: &str) -> bool {
        self.user_ids
            .as_ref()
            .is_none_or(|user_ids| user_ids.iter().any(|candidate| candidate == user_id))
    }
}

#[derive(Clone)]
pub(crate) struct RuntimeSync {
    revision: Arc<AtomicU64>,
    events: broadcast::Sender<RuntimeSyncEvent>,
}

impl Default for RuntimeSync {
    fn default() -> Self {
        let (events, _) = broadcast::channel(RUNTIME_SYNC_BUFFER);
        Self {
            revision: Arc::new(AtomicU64::new(0)),
            events,
        }
    }
}

impl RuntimeSync {
    pub fn notify(&self, topic: RuntimeSyncTopic) {
        self.publish(topic, None);
    }

    pub fn notify_private_messages(&self, user_ids: impl IntoIterator<Item = String>) {
        let mut user_ids = user_ids
            .into_iter()
            .filter(|user_id| !user_id.trim().is_empty())
            .collect::<Vec<_>>();
        user_ids.sort();
        user_ids.dedup();
        if user_ids.is_empty() {
            return;
        }
        self.publish(RuntimeSyncTopic::PrivateMessages, Some(user_ids));
    }

    fn publish(&self, topic: RuntimeSyncTopic, user_ids: Option<Vec<String>>) {
        let revision = self.revision.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.events.send(RuntimeSyncEvent {
            topic,
            revision,
            user_ids,
        });
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RuntimeSyncEvent> {
        self.events.subscribe()
    }

    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_shared_channel_preserves_topics_and_revision_order() {
        let sync = RuntimeSync::default();
        let mut events = sync.subscribe();

        sync.notify(RuntimeSyncTopic::ActiveChatTurns);
        sync.notify(RuntimeSyncTopic::ActiveTerminals);
        sync.notify(RuntimeSyncTopic::AutonomousAgents);
        sync.notify(RuntimeSyncTopic::AccountCompletions);

        assert_eq!(
            events.try_recv().unwrap(),
            RuntimeSyncEvent {
                topic: RuntimeSyncTopic::ActiveChatTurns,
                revision: 1,
                user_ids: None,
            }
        );
        assert_eq!(
            events.try_recv().unwrap(),
            RuntimeSyncEvent {
                topic: RuntimeSyncTopic::ActiveTerminals,
                revision: 2,
                user_ids: None,
            }
        );
        assert_eq!(
            events.try_recv().unwrap(),
            RuntimeSyncEvent {
                topic: RuntimeSyncTopic::AutonomousAgents,
                revision: 3,
                user_ids: None,
            }
        );
        assert_eq!(
            events.try_recv().unwrap(),
            RuntimeSyncEvent {
                topic: RuntimeSyncTopic::AccountCompletions,
                revision: 4,
                user_ids: None,
            }
        );
        assert_eq!(sync.revision(), 4);
    }

    #[test]
    fn private_message_events_are_visible_only_to_their_participants() {
        let sync = RuntimeSync::default();
        let mut events = sync.subscribe();

        sync.notify_private_messages([
            "user-b".to_string(),
            "user-a".to_string(),
            "user-b".to_string(),
        ]);

        let event = events.try_recv().unwrap();
        assert_eq!(event.topic, RuntimeSyncTopic::PrivateMessages);
        assert_eq!(event.revision, 1);
        assert!(event.is_visible_to("user-a"));
        assert!(event.is_visible_to("user-b"));
        assert!(!event.is_visible_to("user-c"));
    }
}
