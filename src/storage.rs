//! Slot storage for keyed cache membership, independent of payload residency.

use std::collections::HashMap;
use std::sync::Arc;

use crate::cache::{SCIdentity, SCStoredPayload};

pub(crate) struct Namespace;
pub(crate) struct IdentityToken {
    pub(crate) namespace: Arc<Namespace>,
    pub(crate) slot: usize,
}

pub(crate) struct Entry<'cleanup, K, T, E> {
    pub(crate) key: K,
    pub(crate) next: Option<usize>,
    pub(crate) identity: SCIdentity<K>,
    pub(crate) state: SCStoredPayload<'cleanup, T, E>,
    pub(crate) policy_bytes: u64,
}

pub(crate) struct Storage<'cleanup, K, T, E> {
    // Rehashing this index cannot invoke user Hash, Eq or Drop. Collision
    // chains in slots still require full semantic equality for every query.
    pub(crate) keys: HashMap<u64, usize>,
    pub(crate) slots: Vec<Option<Entry<'cleanup, K, T, E>>>,
    pub(crate) free: Vec<usize>,
    pub(crate) cursor: usize,
    pub(crate) namespace: Arc<Namespace>,
}

impl<K, T, E> Storage<'_, K, T, E> {
    pub(crate) fn new() -> Self {
        Self {
            keys: HashMap::new(),
            slots: Vec::new(),
            free: Vec::new(),
            cursor: 0,
            namespace: Arc::new(Namespace),
        }
    }
}
