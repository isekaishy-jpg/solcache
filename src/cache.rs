//! Typed keyed membership and protected retained-payload retrieval.

use std::fmt;
use std::hash::{BuildHasher, Hash, Hasher};
use std::marker::PhantomData;
use std::sync::Arc;

use crate::accounting::SCAllocationClass;
use crate::ownership::{SCBacking, SCView};
use crate::storage::{Entry, IdentityToken, Storage};

mod retention;
pub use retention::SCRetentionReport;

/// Cache-scoped logical membership, stable across payload eviction.
///
/// Removal and recreation create distinct identities, including recycled slots.
/// Identity retention does not retain a payload. Key type and cache namespace
/// prevent unrelated memberships from being substituted for one another.
pub struct SCIdentity<K> {
    pub(crate) token: Arc<IdentityToken>,
    key_type: PhantomData<fn() -> K>,
}

impl<K> Clone for SCIdentity<K> {
    fn clone(&self) -> Self {
        Self {
            token: Arc::clone(&self.token),
            key_type: PhantomData,
        }
    }
}
impl<K> PartialEq for SCIdentity<K> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.token, &other.token)
    }
}
impl<K> Eq for SCIdentity<K> {}
impl<K> Hash for SCIdentity<K> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.token).hash(state);
    }
}
impl<K> fmt::Debug for SCIdentity<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SCIdentity")
            .field("token", &Arc::as_ptr(&self.token))
            .finish()
    }
}

/// Retrieval distinguishes usable backing from missing or failed membership.
/// No unavailable outcome creates consumer demand or starts production.
pub enum SCLookup<'entry, R, E> {
    Absent,
    Vacant,
    Failed(&'entry E),
    Ready(R),
}

/// Owned payload state returned by replacement or removal.
/// Dropping a ready state releases its owner using that backing's cleanup contract.
pub enum SCStoredPayload<'cleanup, T, E> {
    Vacant,
    Failed(E),
    Ready(SCBacking<'cleanup, T>),
}
impl<T, E> fmt::Debug for SCStoredPayload<'_, T, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Vacant => "Vacant",
            Self::Failed(_) => "Failed(..)",
            Self::Ready(_) => "Ready(..)",
        })
    }
}

/// Why an installation rejected its original owned inputs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SCInstallErrorReason {
    PolicyOverflow,
    InvalidClass,
}

/// Rejected installation retaining its key, backing and declared policy cost.
pub struct SCInstallError<'cleanup, K, T> {
    key: K,
    backing: SCBacking<'cleanup, T>,
    policy_bytes: u64,
    reason: SCInstallErrorReason,
}

/// Installation result containing stable membership and the displaced owned state.
pub type SCInstallResult<'cleanup, K, T, E = ()> =
    Result<(SCIdentity<K>, SCStoredPayload<'cleanup, T, E>), SCInstallError<'cleanup, K, T>>;

pub(crate) enum IdentityReplaceReason {
    Missing,
    InvalidClass,
    PolicyOverflow,
}

pub(crate) struct IdentityReplaceFailure<'cleanup, T, E> {
    pub(crate) payload: SCStoredPayload<'cleanup, T, E>,
    pub(crate) policy_bytes: u64,
    pub(crate) reason: IdentityReplaceReason,
}

impl<'cleanup, K, T> SCInstallError<'cleanup, K, T> {
    pub fn reason(&self) -> SCInstallErrorReason {
        self.reason
    }
    pub fn into_parts(self) -> (K, SCBacking<'cleanup, T>, u64) {
        (self.key, self.backing, self.policy_bytes)
    }
}
impl<K, T> fmt::Debug for SCInstallError<'_, K, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SCInstallError")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

/// Single-owner typed cache with synchronous operations and no table lock.
///
/// Keys use semantic `Hash` and `Eq`; collisions never substitute for equality.
/// Stored key hashing and equality must remain stable during membership, as for
/// a standard `HashMap`; changing them is a logical error, never memory unsafety.
/// Borrowed lookup protects backing through the cache borrow, while sharing or
/// taking retains independent backing that survives replacement and removal.
/// Mutations require exclusive access. Key callbacks and payload cleanup execute
/// on the caller. This facility creates membership, not production or demand.
/// Backing accounting and deferred cleanup can acquire their own bookkeeping
/// locks; table operations do not promise lock-free execution.
///
/// Each ready slot contributes its supplied policy bytes independently. Aliased
/// backing has one physical charge, but multiple slot owners conservatively pin
/// one another against collection. Slots retain peak membership capacity; bounded
/// maintenance counts empty slots as work rather than hiding an unbounded scan.
pub struct SCCache<'cleanup, K, T, E = ()> {
    pub(super) storage: Storage<'cleanup, K, T, E>,
    pub(super) policy_bytes: u64,
}

impl<'cleanup, K, T, E> SCCache<'cleanup, K, T, E> {
    pub fn new() -> Self {
        Self {
            storage: Storage::new(),
            policy_bytes: 0,
        }
    }
    pub fn len(&self) -> usize {
        self.storage.slots.len() - self.storage.free.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Reports retained slot policy cost, independently of physical accounting.
    pub fn policy_bytes(&self) -> u64 {
        self.policy_bytes
    }
    /// Tests a handle without running key callbacks. Foreign and stale handles fail.
    pub fn contains_identity(&self, identity: &SCIdentity<K>) -> bool {
        self.entry_identity(identity).is_some()
    }
    fn entry_identity(&self, identity: &SCIdentity<K>) -> Option<&Entry<'cleanup, K, T, E>> {
        if !Arc::ptr_eq(&self.storage.namespace, &identity.token.namespace) {
            return None;
        }
        self.storage
            .slots
            .get(identity.token.slot)?
            .as_ref()
            .filter(|entry| entry.identity == *identity)
    }
    /// Borrows ready backing by handle; foreign or removed membership is absent.
    pub fn lookup_identity(&self, identity: &SCIdentity<K>) -> SCLookup<'_, SCView<'_, T>, E> {
        observe(self.entry_identity(identity))
    }

    /// Acquires ready backing by identity without invoking key callbacks.
    /// Foreign or removed membership is absent; the returned owner survives
    /// subsequent replacement, invalidation and table removal.
    pub fn share_identity(
        &self,
        identity: &SCIdentity<K>,
    ) -> SCLookup<'_, SCBacking<'cleanup, T>, E> {
        match self.entry_identity(identity).map(|entry| &entry.state) {
            None => SCLookup::Absent,
            Some(SCStoredPayload::Vacant) => SCLookup::Vacant,
            Some(SCStoredPayload::Failed(error)) => SCLookup::Failed(error),
            Some(SCStoredPayload::Ready(backing)) => SCLookup::Ready(backing.clone()),
        }
    }

    /// Publication commits against a protected identity after validating its
    /// authority. No key callbacks or displaced payload destructors run here.
    /// All fallible checks precede changes; failure returns the original input.
    pub(crate) fn replace_identity(
        &mut self,
        identity: &SCIdentity<K>,
        payload: SCStoredPayload<'cleanup, T, E>,
        policy_bytes: u64,
    ) -> Result<SCStoredPayload<'cleanup, T, E>, IdentityReplaceFailure<'cleanup, T, E>> {
        let Some(entry) = self.entry_identity(identity) else {
            return Err(IdentityReplaceFailure {
                payload,
                policy_bytes,
                reason: IdentityReplaceReason::Missing,
            });
        };
        let ready = if let SCStoredPayload::Ready(backing) = &payload {
            if backing.allocation_class() != SCAllocationClass::Resident {
                return Err(IdentityReplaceFailure {
                    payload,
                    policy_bytes,
                    reason: IdentityReplaceReason::InvalidClass,
                });
            }
            true
        } else {
            false
        };
        let next_cost = if ready { policy_bytes } else { 0 };
        let Some(total) = (self.policy_bytes - entry.policy_bytes).checked_add(next_cost) else {
            return Err(IdentityReplaceFailure {
                payload,
                policy_bytes,
                reason: IdentityReplaceReason::PolicyOverflow,
            });
        };
        let entry = self.storage.slots[identity.token.slot]
            .as_mut()
            .expect("validated identity remains present under exclusive access");
        let previous = std::mem::replace(&mut entry.state, payload);
        entry.policy_bytes = next_cost;
        self.policy_bytes = total;
        Ok(previous)
    }
}
impl<K, T, E> Default for SCCache<'_, K, T, E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'cleanup, K: Eq + Hash, T, E> SCCache<'cleanup, K, T, E> {
    /// Creates vacant membership only; an existing failure or payload is preserved.
    pub fn ensure(&mut self, key: K) -> SCIdentity<K> {
        let hash = self.storage.keys.hasher().hash_one(&key);
        let slot = self.find_hashed(hash, &key).map(|(slot, _)| slot);
        self.ensure_hashed(key, hash, slot)
    }

    fn find_hashed(&self, hash: u64, key: &K) -> Option<(usize, Option<usize>)> {
        let mut slot = self.storage.keys.get(&hash).copied();
        let mut previous = None;
        while let Some(index) = slot {
            let entry = self.storage.slots[index]
                .as_ref()
                .expect("bucket owns a live slot");
            if entry.key == *key {
                return Some((index, previous));
            }
            previous = Some(index);
            slot = entry.next;
        }
        None
    }

    fn find(&self, key: &K) -> Option<usize> {
        let hash = self.storage.keys.hasher().hash_one(key);
        self.find_hashed(hash, key).map(|(slot, _)| slot)
    }

    fn ensure_hashed(&mut self, key: K, hash: u64, existing: Option<usize>) -> SCIdentity<K> {
        if let Some(slot) = existing {
            // An incoming duplicate key may run arbitrary Drop; do it before mutation.
            drop(key);
            return self.storage.slots[slot]
                .as_ref()
                .expect("key owns a live slot")
                .identity
                .clone();
        }
        self.storage.keys.reserve(1);
        if self.storage.free.is_empty() {
            self.storage.slots.reserve(1);
        }
        let slot = self
            .storage
            .free
            .last()
            .copied()
            .unwrap_or(self.storage.slots.len());
        let identity = SCIdentity {
            token: Arc::new(IdentityToken {
                namespace: Arc::clone(&self.storage.namespace),
                slot,
            }),
            key_type: PhantomData,
        };
        let next = self.storage.keys.get(&hash).copied();
        let entry = Some(Entry {
            key,
            next,
            identity: identity.clone(),
            state: SCStoredPayload::Vacant,
            policy_bytes: 0,
        });
        if slot == self.storage.slots.len() {
            self.storage.slots.push(entry);
        } else {
            self.storage.free.pop();
            self.storage.slots[slot] = entry;
        }
        // Capacity is reserved and the index contains only cached integer hashes.
        // No user callbacks occur while linking a new slot into the index.
        self.storage.keys.insert(hash, slot);
        identity
    }
    pub fn identity(&self, key: &K) -> Option<SCIdentity<K>> {
        let slot = self.find(key)?;
        Some(
            self.storage.slots[slot]
                .as_ref()
                .expect("key owns a live slot")
                .identity
                .clone(),
        )
    }
    /// Observes readiness without cloning backing or creating demand.
    pub fn lookup(&self, key: &K) -> SCLookup<'_, SCView<'_, T>, E> {
        observe(
            self.find(key)
                .and_then(|slot| self.storage.slots[slot].as_ref()),
        )
    }
    /// Acquires an existing ready owner under a protected cache borrow.
    pub fn share(&self, key: &K) -> SCLookup<'_, SCBacking<'cleanup, T>, E> {
        let Some(slot) = self.find(key) else {
            return SCLookup::Absent;
        };
        match &self.storage.slots[slot]
            .as_ref()
            .expect("key owns a live slot")
            .state
        {
            SCStoredPayload::Vacant => SCLookup::Vacant,
            SCStoredPayload::Failed(error) => SCLookup::Failed(error),
            SCStoredPayload::Ready(backing) => SCLookup::Ready(backing.clone()),
        }
    }
    /// Moves the table owner and its physical charge, leaving logical membership vacant.
    pub fn take(&mut self, key: &K) -> SCLookup<'_, SCBacking<'cleanup, T>, E> {
        let Some(slot) = self.find(key) else {
            return SCLookup::Absent;
        };
        let entry = self.storage.slots[slot]
            .as_mut()
            .expect("key owns a live slot");
        if matches!(entry.state, SCStoredPayload::Ready(_)) {
            self.policy_bytes -= entry.policy_bytes;
            entry.policy_bytes = 0;
            let SCStoredPayload::Ready(backing) =
                std::mem::replace(&mut entry.state, SCStoredPayload::Vacant)
            else {
                unreachable!()
            };
            return SCLookup::Ready(backing);
        }
        match &entry.state {
            SCStoredPayload::Failed(error) => SCLookup::Failed(error),
            _ => SCLookup::Vacant,
        }
    }
    /// Installs resident backing, returning the previous owned state.
    /// Rejection preserves inputs and membership. Escaped old owners stay valid.
    pub fn install(
        &mut self,
        key: K,
        backing: SCBacking<'cleanup, T>,
        policy_bytes: u64,
    ) -> SCInstallResult<'cleanup, K, T, E> {
        if backing.allocation_class() != SCAllocationClass::Resident {
            return Err(SCInstallError {
                key,
                backing,
                policy_bytes,
                reason: SCInstallErrorReason::InvalidClass,
            });
        }
        let hash = self.storage.keys.hasher().hash_one(&key);
        let slot = self.find_hashed(hash, &key).map(|(slot, _)| slot);
        let old_cost = slot.map_or(0, |slot| {
            self.storage.slots[slot]
                .as_ref()
                .expect("key owns a live slot")
                .policy_bytes
        });
        let Some(total) = (self.policy_bytes - old_cost).checked_add(policy_bytes) else {
            return Err(SCInstallError {
                key,
                backing,
                policy_bytes,
                reason: SCInstallErrorReason::PolicyOverflow,
            });
        };
        let identity = self.ensure_hashed(key, hash, slot);
        let entry = self.storage.slots[identity.token.slot]
            .as_mut()
            .expect("ensured slot is live");
        let previous = std::mem::replace(&mut entry.state, SCStoredPayload::Ready(backing));
        entry.policy_bytes = policy_bytes;
        self.policy_bytes = total;
        Ok((identity, previous))
    }
    /// Stores a domain failure explicitly; failure membership is never a usable hit.
    pub fn fail(&mut self, key: K, error: E) -> (SCIdentity<K>, SCStoredPayload<'cleanup, T, E>) {
        let identity = self.ensure(key);
        let previous = self.replace_slot(identity.token.slot, SCStoredPayload::Failed(error));
        (identity, previous)
    }
    /// Detaches payload or failure without removing the identity.
    pub fn clear_payload(&mut self, key: &K) -> Option<SCStoredPayload<'cleanup, T, E>> {
        let slot = self.find(key)?;
        Some(self.replace_slot(slot, SCStoredPayload::Vacant))
    }
    fn replace_slot(
        &mut self,
        slot: usize,
        state: SCStoredPayload<'cleanup, T, E>,
    ) -> SCStoredPayload<'cleanup, T, E> {
        let entry = self.storage.slots[slot]
            .as_mut()
            .expect("key owns a live slot");
        self.policy_bytes -= entry.policy_bytes;
        entry.policy_bytes = 0;
        std::mem::replace(&mut entry.state, state)
    }
    /// Removes membership and returns its state. Independent owners survive removal.
    pub fn remove(&mut self, key: &K) -> Option<(SCIdentity<K>, SCStoredPayload<'cleanup, T, E>)> {
        let entry = self.remove_entry(key)?;
        // The table and counters are coherent before arbitrary key destruction.
        drop(entry.key);
        Some((entry.identity, entry.state))
    }

    /// Detaches membership without running key or payload destructors. Layered
    /// coordinators can settle their own metadata before user cleanup can unwind.
    /// Hash/Eq callbacks still run before any membership change.
    pub(crate) fn remove_entry(&mut self, key: &K) -> Option<Entry<'cleanup, K, T, E>> {
        let hash = self.storage.keys.hasher().hash_one(key);
        let (slot, previous) = self.find_hashed(hash, key)?;
        self.storage.free.reserve(1);
        let entry = self.storage.slots[slot]
            .take()
            .expect("removed key owns a live slot");
        if let Some(previous) = previous {
            self.storage.slots[previous]
                .as_mut()
                .expect("chain predecessor is live")
                .next = entry.next;
        } else if let Some(next) = entry.next {
            self.storage.keys.insert(hash, next);
        } else {
            self.storage.keys.remove(&hash);
        }
        self.policy_bytes -= entry.policy_bytes;
        self.storage.free.push(slot);
        Some(entry)
    }
}

fn observe<'entry, K, T, E>(
    entry: Option<&'entry Entry<'_, K, T, E>>,
) -> SCLookup<'entry, SCView<'entry, T>, E> {
    match entry.map(|entry| &entry.state) {
        None => SCLookup::Absent,
        Some(SCStoredPayload::Vacant) => SCLookup::Vacant,
        Some(SCStoredPayload::Failed(error)) => SCLookup::Failed(error),
        Some(SCStoredPayload::Ready(backing)) => SCLookup::Ready(backing.view()),
    }
}
