//! Owner-driven publication with retained readers and declared validity.

use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;
use std::sync::Arc;

use crate::cache::{
    IdentityReplaceReason, SCCache, SCIdentity, SCInstallErrorReason, SCLookup, SCRetentionReport,
    SCStoredPayload,
};
use crate::dependency::{SCDependencies, SCDependencyError, SCDependencySnapshot};
use crate::ownership::{SCBacking, SCView};
use crate::policy::SCByteLimit;

#[cfg(test)]
#[path = "../tests/unit/publication.rs"]
mod tests;

struct State {
    authority: Arc<()>,
    round: Option<Arc<()>>,
    validity: Option<SCDependencySnapshot>,
}

/// Local authority attachment, not proof of transport provenance.
/// Providers must bind this token to the actual delivery context before switching
/// authorities, or settle the old stream first.
pub struct SCAuthority<K> {
    identity: SCIdentity<K>,
    token: Arc<()>,
}
impl<K> Clone for SCAuthority<K> {
    fn clone(&self) -> Self {
        Self {
            identity: self.identity.clone(),
            token: Arc::clone(&self.token),
        }
    }
}

/// Captured owner attachment and declared dependencies for one publication round.
/// Clone only when the domain permits independent candidates: clones contend for
/// one winner. The first successful publish closes the round; cloning does not
/// start production or create a new round. Pair each ticket with its own production outcome. Publication must
/// receive an owned outcome claimed after all producer accesses have ended.
pub struct SCPublicationAttempt<K> {
    authority: SCAuthority<K>,
    round: Arc<()>,
    validity: SCDependencySnapshot,
}
impl<K> Clone for SCPublicationAttempt<K> {
    fn clone(&self) -> Self {
        Self {
            authority: self.authority.clone(),
            round: Arc::clone(&self.round),
            validity: self.validity.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SCPublicationRejectionReason {
    Missing,
    AuthorityChanged,
    ObsoleteAttempt,
    Dependency(SCDependencyError),
    Install(SCInstallErrorReason),
}

/// Rejection preserves the supplied owned payload and policy cost for caller cleanup.
pub struct SCPublicationRejected<'cleanup, T, E> {
    payload: Result<SCBacking<'cleanup, T>, E>,
    policy_bytes: u64,
    reason: SCPublicationRejectionReason,
}
impl<'cleanup, T, E> SCPublicationRejected<'cleanup, T, E> {
    pub fn reason(&self) -> SCPublicationRejectionReason {
        self.reason
    }
    pub fn into_parts(self) -> (Result<SCBacking<'cleanup, T>, E>, u64) {
        (self.payload, self.policy_bytes)
    }
}
impl<T, E> fmt::Debug for SCPublicationRejected<'_, T, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SCPublicationRejected")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

/// Single-owner cache coordinator. Exclusive mutation makes validation and install
/// one protected phase without a publication lock or delivery executor. No user
/// callback or destructor runs between successful install and authority commit.
/// Invalidating a declared dependency prevents subsequent lookup/publication but
/// does not revoke escaped readers. Domain code explicitly chooses propagation.
pub struct SCPublication<'cleanup, K, T, E = ()> {
    cache: SCCache<'cleanup, K, T, E>,
    states: HashMap<SCIdentity<K>, State>,
}
impl<'cleanup, K, T, E> SCPublication<'cleanup, K, T, E> {
    pub fn new() -> Self {
        Self {
            cache: SCCache::new(),
            states: HashMap::new(),
        }
    }
    pub fn policy_bytes(&self) -> u64 {
        self.cache.policy_bytes()
    }
    /// Runs the existing bounded retention service on the caller. Eviction keeps
    /// attempt attachment and logical identity; vacant entries have no validity
    /// requirement until a new payload is successfully installed.
    pub fn maintain(
        &mut self,
        limit: SCByteLimit,
        max_slots: usize,
        force: bool,
    ) -> SCRetentionReport {
        self.cache.maintain(limit, max_slots, force)
    }
    /// Consumes validity bookkeeping. The returned raw cache no longer checks it.
    pub fn into_cache(self) -> SCCache<'cleanup, K, T, E> {
        self.cache
    }
    pub fn authority(&self, identity: &SCIdentity<K>) -> Option<SCAuthority<K>> {
        if !self.cache.contains_identity(identity) {
            return None;
        }
        self.states.get(identity).map(|state| SCAuthority {
            identity: identity.clone(),
            token: Arc::clone(&state.authority),
        })
    }
    /// Starts a fresh round without erasing the existing usable payload.
    /// Capturing current inputs and revisions is the caller's responsibility;
    /// this operation replaces the attachment even if the supplied snapshot is
    /// stale. Declared validity is checked when publishing the owned outcome.
    /// This is not a join operation. With `SCProductionMap::begin_shared`, call
    /// this only for `Started` and before provider submission; a `Joined` caller
    /// must preserve the existing round. Starting a round does not replace or
    /// cancel the production map's associated work. Never attach a fresh ticket
    /// to an old outcome to bypass its captured dependency validity.
    pub fn begin_attempt(
        &mut self,
        identity: &SCIdentity<K>,
        validity: SCDependencySnapshot,
    ) -> Option<SCPublicationAttempt<K>> {
        if !self.cache.contains_identity(identity) {
            return None;
        }
        let state = self.states.get_mut(identity)?;
        let round = Arc::new(());
        state.round = Some(Arc::clone(&round));
        Some(SCPublicationAttempt {
            authority: SCAuthority {
                identity: identity.clone(),
                token: Arc::clone(&state.authority),
            },
            round,
            validity,
        })
    }
    fn validate_stored(
        &self,
        identity: &SCIdentity<K>,
        dependencies: &SCDependencies,
    ) -> Result<(), SCDependencyError> {
        if matches!(
            self.cache.lookup_identity(identity),
            SCLookup::Absent | SCLookup::Vacant
        ) {
            return Ok(());
        }
        if let Some(validity) = self
            .states
            .get(identity)
            .and_then(|state| state.validity.as_ref())
        {
            dependencies.validate(validity)?;
        }
        Ok(())
    }
    pub fn lookup(
        &self,
        identity: &SCIdentity<K>,
        dependencies: &SCDependencies,
    ) -> Result<SCLookup<'_, SCView<'_, T>, E>, SCDependencyError> {
        self.validate_stored(identity, dependencies)?;
        Ok(self.cache.lookup_identity(identity))
    }
    pub fn share(
        &self,
        identity: &SCIdentity<K>,
        dependencies: &SCDependencies,
    ) -> Result<SCLookup<'_, SCBacking<'cleanup, T>, E>, SCDependencyError> {
        self.validate_stored(identity, dependencies)?;
        Ok(self.cache.share_identity(identity))
    }
    /// Clears payload and attempt attachment while preserving delivery authority.
    /// Untagged updates vouched for by the caller may repopulate this membership.
    pub fn clear(&mut self, identity: &SCIdentity<K>) -> Option<SCStoredPayload<'cleanup, T, E>> {
        let state = self.states.get_mut(identity)?;
        let previous = self
            .cache
            .replace_identity(identity, SCStoredPayload::Vacant, 0)
            .ok()?;
        state.round = None;
        state.validity = None;
        Some(previous)
    }
    /// Replaces the source/delivery authority and clears payload. Old tagged
    /// deliveries and attempt tickets are rejected; old backing readers survive.
    pub fn replace_authority(
        &mut self,
        identity: &SCIdentity<K>,
    ) -> Option<SCStoredPayload<'cleanup, T, E>> {
        let authority = Arc::new(());
        let previous = self.clear(identity)?;
        self.states
            .get_mut(identity)
            .expect("cleared membership exists")
            .authority = authority;
        Some(previous)
    }
    pub fn publish(
        &mut self,
        attempt: &SCPublicationAttempt<K>,
        dependencies: &SCDependencies,
        payload: Result<SCBacking<'cleanup, T>, E>,
        policy_bytes: u64,
    ) -> Result<SCStoredPayload<'cleanup, T, E>, SCPublicationRejected<'cleanup, T, E>> {
        let identity = &attempt.authority.identity;
        let rejection = match self.states.get(identity) {
            _ if !self.cache.contains_identity(identity) => {
                Some(SCPublicationRejectionReason::Missing)
            }
            None => Some(SCPublicationRejectionReason::Missing),
            Some(state) if !Arc::ptr_eq(&state.authority, &attempt.authority.token) => {
                Some(SCPublicationRejectionReason::AuthorityChanged)
            }
            Some(state)
                if !state
                    .round
                    .as_ref()
                    .is_some_and(|round| Arc::ptr_eq(round, &attempt.round)) =>
            {
                Some(SCPublicationRejectionReason::ObsoleteAttempt)
            }
            _ => None,
        };
        if let Some(reason) = rejection {
            return Err(SCPublicationRejected {
                payload,
                policy_bytes,
                reason,
            });
        }
        self.install(
            identity,
            dependencies,
            &attempt.validity,
            payload,
            policy_bytes,
        )
    }
    /// Applies an authoritative update independently of attempt completion.
    /// `None` means the caller vouches for the current delivery authority; SC
    /// cannot distinguish old untagged transport messages or invent provenance.
    /// A successful update revokes pending attempt attachment. Rejection changes
    /// neither payload nor attachment and returns the supplied outcome intact.
    /// The validity stamp is borrowed so a rejected update can retry with its
    /// original revisions; do not recapture newer revisions for an old payload.
    pub fn authoritative(
        &mut self,
        identity: &SCIdentity<K>,
        authority: Option<&SCAuthority<K>>,
        dependencies: &SCDependencies,
        validity: &SCDependencySnapshot,
        payload: Result<SCBacking<'cleanup, T>, E>,
        policy_bytes: u64,
    ) -> Result<SCStoredPayload<'cleanup, T, E>, SCPublicationRejected<'cleanup, T, E>> {
        let rejection = match self.states.get(identity) {
            _ if !self.cache.contains_identity(identity) => {
                Some(SCPublicationRejectionReason::Missing)
            }
            None => Some(SCPublicationRejectionReason::Missing),
            Some(state)
                if authority.is_some_and(|authority| {
                    authority.identity != *identity
                        || !Arc::ptr_eq(&authority.token, &state.authority)
                }) =>
            {
                Some(SCPublicationRejectionReason::AuthorityChanged)
            }
            _ => None,
        };
        if let Some(reason) = rejection {
            return Err(SCPublicationRejected {
                payload,
                policy_bytes,
                reason,
            });
        }
        self.install(identity, dependencies, validity, payload, policy_bytes)
    }
    fn install(
        &mut self,
        identity: &SCIdentity<K>,
        dependencies: &SCDependencies,
        validity: &SCDependencySnapshot,
        payload: Result<SCBacking<'cleanup, T>, E>,
        policy_bytes: u64,
    ) -> Result<SCStoredPayload<'cleanup, T, E>, SCPublicationRejected<'cleanup, T, E>> {
        if let Err(error) = dependencies.validate(validity) {
            return Err(SCPublicationRejected {
                payload,
                policy_bytes,
                reason: SCPublicationRejectionReason::Dependency(error),
            });
        }
        let validity = validity.clone();
        let stored = match payload {
            Ok(backing) => SCStoredPayload::Ready(backing),
            Err(error) => SCStoredPayload::Failed(error),
        };
        let previous = self
            .cache
            .replace_identity(identity, stored, policy_bytes)
            .map_err(|error| SCPublicationRejected {
                payload: match error.payload {
                    SCStoredPayload::Ready(backing) => Ok(backing),
                    SCStoredPayload::Failed(error) => Err(error),
                    SCStoredPayload::Vacant => unreachable!("production outcome is never vacant"),
                },
                policy_bytes: error.policy_bytes,
                reason: match error.reason {
                    IdentityReplaceReason::Missing => SCPublicationRejectionReason::Missing,
                    IdentityReplaceReason::InvalidClass => {
                        SCPublicationRejectionReason::Install(SCInstallErrorReason::InvalidClass)
                    }
                    IdentityReplaceReason::PolicyOverflow => {
                        SCPublicationRejectionReason::Install(SCInstallErrorReason::PolicyOverflow)
                    }
                },
            })?;
        let state = self
            .states
            .get_mut(identity)
            .expect("validated membership exists");
        state.validity = Some(validity);
        state.round = None;
        Ok(previous)
    }
}
impl<K, T, E> Default for SCPublication<'_, K, T, E> {
    fn default() -> Self {
        Self::new()
    }
}
impl<'cleanup, K: Eq + Hash, T, E> SCPublication<'cleanup, K, T, E> {
    pub fn ensure(&mut self, key: K) -> SCIdentity<K> {
        let identity = self.cache.ensure(key);
        self.states
            .entry(identity.clone())
            .or_insert_with(|| State {
                authority: Arc::new(()),
                round: None,
                validity: None,
            });
        identity
    }
    /// Removes membership. Escaped backing and old production remain owned.
    /// Key callbacks run on the caller. Membership and publication metadata are
    /// detached before key destruction, including when that destructor panics.
    pub fn remove(&mut self, key: &K) -> Option<(SCIdentity<K>, SCStoredPayload<'cleanup, T, E>)> {
        let entry = self.cache.remove_entry(key)?;
        self.states.remove(&entry.identity);
        drop(entry.key);
        Some((entry.identity, entry.state))
    }
    pub fn identity(&self, key: &K) -> Option<SCIdentity<K>> {
        self.cache.identity(key)
    }
}
