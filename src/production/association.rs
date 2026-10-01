//! Owner-driven association; this does not confer publication authority.

use std::collections::HashMap;
use std::fmt;

use crate::cache::SCIdentity;
use crate::demand::{SCDemand, SCDemandHandle};
use crate::policy::SCAdmission;

use super::{SCProduction, SCProductionPhase, SCSubmission};

struct Association<O, E> {
    demand: SCDemand,
    current: Option<SCProduction<O, E>>,
}

/// Explicit consumer interest and production association by cache-scoped identity.
///
/// Mutation requires an owner or caller synchronization. Internal map hashing
/// uses identity tokens, never user key callbacks. Operations may allocate and
/// synchronously lock demand, admission and attempt bookkeeping in sequence.
/// Identity membership and publication validity remain the caller's concern.
/// Removing an association or dropping this map never cancels escaped work.
pub struct SCProductionMap<K, O, E> {
    associations: HashMap<SCIdentity<K>, Association<O, E>>,
    admission: SCAdmission,
}

impl<K, O, E> SCProductionMap<K, O, E> {
    pub fn new(admission: SCAdmission) -> Self {
        Self {
            associations: HashMap::new(),
            admission,
        }
    }

    /// Creates independent interest, without starting production.
    pub fn interest(
        &mut self,
        identity: SCIdentity<K>,
        revision: u64,
        urgency: u32,
    ) -> SCDemandHandle {
        self.associations
            .entry(identity)
            .or_insert_with(|| Association {
                demand: SCDemand::new(),
                current: None,
            })
            .demand
            .attach(revision, urgency)
    }

    /// Observes demand without creating another consumer.
    pub fn demand(&self, identity: &SCIdentity<K>) -> Option<SCDemand> {
        self.associations
            .get(identity)
            .map(|entry| entry.demand.clone())
    }

    pub fn current(&self, identity: &SCIdentity<K>) -> Option<SCProduction<O, E>> {
        self.associations
            .get(identity)
            .and_then(|entry| entry.current.clone())
    }

    /// Joins pending or unclaimed completed work, returning unused input intact.
    /// Otherwise explicitly starts one retry with a fresh allowance and state.
    /// Rejection, abort, claimed outcome and settled abandonment permit retry.
    /// Demand is checked at admission; later detachment does not cancel work.
    /// Claiming an outcome ends coalescing for that attempt. Callers coordinate
    /// subsequent cache lookup/publication themselves.
    /// A joined observer does not reserve the result: another observer can claim
    /// it concurrently. Serialize claim and publication in the owning coordinator.
    pub fn begin_shared<I>(
        &mut self,
        identity: &SCIdentity<K>,
        inputs: I,
    ) -> Result<SCProductionStart<I, O, E>, SCStartRejected<I>> {
        let Some(entry) = self.associations.get_mut(identity) else {
            return Err(SCStartRejected {
                inputs,
                reason: SCStartRejectionReason::NoDemand,
            });
        };
        if entry.demand.snapshot().consumers() == 0 {
            return Err(SCStartRejected {
                inputs,
                reason: SCStartRejectionReason::NoDemand,
            });
        }
        if let Some(production) = &entry.current {
            let state = production.snapshot();
            let join = match state.phase {
                SCProductionPhase::Prepared => true,
                SCProductionPhase::Accepted => {
                    state.active_accesses != 0
                        || (state.completion_recorded && !state.outcome_claimed)
                }
                SCProductionPhase::Rejected | SCProductionPhase::Aborted => false,
            };
            if join {
                return Ok(SCProductionStart::Joined {
                    production: production.clone(),
                    unused_input: inputs,
                });
            }
        }
        let attempt = prepare(&self.admission, inputs)?;
        entry.current = Some(attempt.production.clone());
        Ok(SCProductionStart::Started(attempt))
    }

    /// Starts a separate owned candidate without changing shared association.
    /// The caller selects a domain permitting independent candidates and later
    /// arbitrates publication; this operation does not select a winner.
    pub fn begin_candidate<I>(
        &self,
        identity: &SCIdentity<K>,
        inputs: I,
    ) -> Result<SCProductionAttempt<I, O, E>, SCStartRejected<I>> {
        if !self
            .associations
            .get(identity)
            .is_some_and(|entry| entry.demand.snapshot().consumers() != 0)
        {
            return Err(SCStartRejected {
                inputs,
                reason: SCStartRejectionReason::NoDemand,
            });
        }
        prepare(&self.admission, inputs)
    }

    /// Removes association only. Existing consumers and access owners stay valid;
    /// later interest for this identity creates a new demand group.
    /// Dropping the final unclaimed outcome can invoke its destructor here.
    pub fn remove(&mut self, identity: &SCIdentity<K>) -> bool {
        self.associations.remove(identity).is_some()
    }
}

fn prepare<I, O, E>(
    admission: &SCAdmission,
    inputs: I,
) -> Result<SCProductionAttempt<I, O, E>, SCStartRejected<I>> {
    let permit = match admission.try_acquire() {
        Ok(permit) => permit,
        Err(_) => {
            return Err(SCStartRejected {
                inputs,
                reason: SCStartRejectionReason::AtCapacity,
            });
        }
    };
    let (production, submission) = SCProduction::prepare_with_permit(inputs, permit);
    Ok(SCProductionAttempt {
        production,
        submission,
    })
}

/// Owned submission paired with its already-installed observer.
pub struct SCProductionAttempt<I, O, E> {
    pub production: SCProduction<O, E>,
    pub submission: SCSubmission<I, O, E>,
}

/// Shared work never consumes the joining caller's unused input.
pub enum SCProductionStart<I, O, E> {
    Started(SCProductionAttempt<I, O, E>),
    Joined {
        production: SCProduction<O, E>,
        unused_input: I,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SCStartRejectionReason {
    NoDemand,
    AtCapacity,
}

/// Admission failure preserving the input for retry or caller cleanup.
pub struct SCStartRejected<I> {
    inputs: I,
    reason: SCStartRejectionReason,
}

impl<I> SCStartRejected<I> {
    pub fn reason(&self) -> SCStartRejectionReason {
        self.reason
    }
    pub fn into_inputs(self) -> I {
        self.inputs
    }
}

impl<I> fmt::Debug for SCStartRejected<I> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SCStartRejected")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}
