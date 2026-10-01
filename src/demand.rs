//! Independent consumer interest with ordered, caller-supplied urgency updates.

use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, MutexGuard};

struct ConsumerToken;
struct RevisionToken;

/// Stable identity of one independently retained consumer interest.
///
/// Cloning this token does not retain demand. After its handle detaches, delayed
/// updates using this identity are ignored. Tokens have no wrapping counter and
/// cannot acquire authority over a later consumer or a different demand group.
#[derive(Clone)]
pub struct SCConsumerIdentity {
    token: Arc<ConsumerToken>,
}

impl PartialEq for SCConsumerIdentity {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.token, &other.token)
    }
}

impl Eq for SCConsumerIdentity {}

impl Hash for SCConsumerIdentity {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.token).hash(state);
    }
}

impl fmt::Debug for SCConsumerIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SCConsumerIdentity")
            .field("token", &Arc::as_ptr(&self.token))
            .finish()
    }
}

struct Interest {
    revision: u64,
    urgency: u32,
}

struct DemandState {
    consumers: HashMap<SCConsumerIdentity, Interest>,
    revision: Arc<RevisionToken>,
}

impl Default for DemandState {
    fn default() -> Self {
        Self {
            consumers: HashMap::new(),
            revision: Arc::new(RevisionToken),
        }
    }
}

/// The outcome of an explicitly ordered consumer urgency update.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SCUrgencyUpdate {
    /// A strictly newer revision replaced the consumer's urgency.
    Applied,
    /// The revision was equal to or older than the consumer's current revision.
    Obsolete,
    /// The consumer has detached or belongs to another demand group.
    Detached,
}

/// A coherent observation of current consumers and their maximum urgency.
///
/// No consumers produce `None`; an attached consumer with urgency zero produces
/// `Some(0)`. Observation performs no production, retry, cancellation or cleanup.
#[derive(Clone)]
pub struct SCDemandSnapshot {
    consumers: usize,
    urgency: Option<u32>,
    revision: Arc<RevisionToken>,
}

impl SCDemandSnapshot {
    /// The number of independently attached consumers at observation time.
    pub fn consumers(&self) -> usize {
        self.consumers
    }

    /// The maximum urgency, or `None` when no consumers were attached.
    pub fn urgency(&self) -> Option<u32> {
        self.urgency
    }
}

impl fmt::Debug for SCDemandSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SCDemandSnapshot")
            .field("consumers", &self.consumers)
            .field("urgency", &self.urgency)
            .finish()
    }
}

/// A shared group of independent consumer interests, without execution services.
///
/// Cloning the group creates an observation/update handle, not consumer demand.
/// Each explicit [`Self::attach`] creates one independently owned consumer. A
/// producer can retain this group without retaining any consumers. Rejection or
/// completion of a producer attempt does not change consumer interests here;
/// retry remains an explicit application operation.
///
/// Operations synchronously acquire a bookkeeping mutex and may block. Attach
/// allocates consumer metadata and may grow the table. Every attach, applied
/// update and detach allocates a fresh revision token; this provides nonwrapping
/// snapshot identity rather than a bounded numeric serial. Only private token
/// hashing and primitive bookkeeping execute under the lock; no user callbacks
/// run there.
#[derive(Clone, Default)]
pub struct SCDemand {
    state: Arc<Mutex<DemandState>>,
}

impl SCDemand {
    /// Creates an empty demand group with no consumer interest.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates one independent consumer at the supplied initial revision.
    ///
    /// Larger urgency values have higher priority. Revisions are supplied by the
    /// caller and never incremented internally; updates must be strictly newer.
    /// Starting at `u64::MAX` permits no later update to this consumer, without
    /// wrapping into authority over an earlier revision.
    pub fn attach(&self, revision: u64, urgency: u32) -> SCDemandHandle {
        let identity = SCConsumerIdentity {
            token: Arc::new(ConsumerToken),
        };
        let mut state = self.lock();
        state
            .consumers
            .insert(identity.clone(), Interest { revision, urgency });
        state.revision = Arc::new(RevisionToken);
        SCDemandHandle {
            demand: self.clone(),
            identity,
        }
    }

    /// Applies a delayed update only to this group's still-attached consumer.
    ///
    /// Equal and older revisions never overwrite newer urgency, including a
    /// decrease in urgency. Detached and foreign identities leave state unchanged.
    pub fn update(
        &self,
        identity: &SCConsumerIdentity,
        revision: u64,
        urgency: u32,
    ) -> SCUrgencyUpdate {
        let mut state = self.lock();
        let Some(interest) = state.consumers.get_mut(identity) else {
            return SCUrgencyUpdate::Detached;
        };
        if revision <= interest.revision {
            return SCUrgencyUpdate::Obsolete;
        }
        interest.revision = revision;
        interest.urgency = urgency;
        state.revision = Arc::new(RevisionToken);
        SCUrgencyUpdate::Applied
    }

    /// Observes aggregate demand, scanning the current consumers under the lock.
    ///
    /// Cost is linear in table capacity, as with standard `HashMap` iteration;
    /// this is not a bounded maintenance pass or a command to an executor.
    pub fn snapshot(&self) -> SCDemandSnapshot {
        let state = self.lock();
        SCDemandSnapshot {
            consumers: state.consumers.len(),
            urgency: state
                .consumers
                .values()
                .map(|interest| interest.urgency)
                .max(),
            revision: Arc::clone(&state.revision),
        }
    }

    /// Whether this observation's token still matches the group's current state.
    ///
    /// Any attach, applied update or detach invalidates previous snapshots, even
    /// if the aggregate urgency remains equal. Foreign snapshots return false.
    /// This is a point-in-time observation under the bookkeeping lock: another
    /// thread can change demand after return. It does not serialize later executor
    /// application or grant producer-attempt authority.
    pub fn is_current(&self, snapshot: &SCDemandSnapshot) -> bool {
        Arc::ptr_eq(&self.lock().revision, &snapshot.revision)
    }

    fn lock(&self) -> MutexGuard<'_, DemandState> {
        // Only private, callback-free bookkeeping runs under this mutex.
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }
}

/// One independent consumer interest, detached on explicit release or final drop.
///
/// The handle deliberately cannot be cloned. Attach another consumer explicitly
/// when another independently retained interest is needed. Dropping this handle
/// never cancels a producer or releases any producer-owned input or backing.
pub struct SCDemandHandle {
    demand: SCDemand,
    identity: SCConsumerIdentity,
}

impl SCDemandHandle {
    /// Returns a stable token for delayed updates, without retaining interest.
    pub fn identity(&self) -> SCConsumerIdentity {
        self.identity.clone()
    }

    /// Updates this consumer only when the supplied revision is strictly newer.
    pub fn update(&self, revision: u64, urgency: u32) -> SCUrgencyUpdate {
        self.demand.update(&self.identity, revision, urgency)
    }

    /// Releases this consumer interest immediately; other consumers remain.
    pub fn detach(self) {
        drop(self);
    }
}

impl Drop for SCDemandHandle {
    fn drop(&mut self) {
        let mut state = self.demand.lock();
        state.consumers.remove(&self.identity);
        state.revision = Arc::new(RevisionToken);
    }
}
