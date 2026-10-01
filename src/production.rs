//! Owned production attempts with provider acceptance and final-access settlement.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::policy::SCAdmissionPermit;

mod association;
pub use association::{
    SCProductionAttempt, SCProductionMap, SCProductionStart, SCStartRejected,
    SCStartRejectionReason,
};

/// Actual provider acceptance, independent of completion and consumer demand.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SCProductionPhase {
    Prepared,
    Accepted,
    Rejected,
    Aborted,
}

/// Observation of one attempt; counts represent SC access capabilities only.
/// No snapshot certifies physical provider or device quiescence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SCProductionSnapshot {
    pub phase: SCProductionPhase,
    pub active_accesses: usize,
    pub completion_recorded: bool,
    pub outcome_claimed: bool,
}

/// Owned outcome retrieval, gated by acceptance and all SC final-access releases.
pub enum SCProductionClaim<O, E> {
    Pending,
    Ready(Result<O, E>),
    Abandoned,
    Rejected,
    Aborted,
    Taken,
}

struct State<O, E> {
    phase: SCProductionPhase,
    active_accesses: usize,
    outcome: Option<Result<O, E>>,
    completion_recorded: bool,
    outcome_claimed: bool,
    permit: Option<SCAdmissionPermit>,
}

impl<O, E> State<O, E> {
    fn settled_permit(&mut self) -> Option<SCAdmissionPermit> {
        if self.phase != SCProductionPhase::Prepared && self.active_accesses == 0 {
            self.permit.take()
        } else {
            None
        }
    }
}

fn lock<O, E>(state: &Mutex<State<O, E>>) -> MutexGuard<'_, State<O, E>> {
    // Guards protect callback-free bookkeeping only. Poison recovery preserves
    // owned attempts during unrelated unwinding rather than dropping obligations.
    state.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Shared observation of a prepared production attempt.
/// Cloning does not add producer access, demand or completion authority.
/// Methods synchronously acquire a bookkeeping mutex; no executor is required.
pub struct SCProduction<O, E> {
    state: Arc<Mutex<State<O, E>>>,
}
impl<O, E> Clone for SCProduction<O, E> {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
        }
    }
}
impl<O, E> SCProduction<O, E> {
    /// Installs observation and callback state before any provider invocation.
    /// The returned submission owns inputs until explicit submission or drop.
    pub fn prepare<I>(inputs: I) -> (Self, SCSubmission<I, O, E>) {
        Self::prepare_owned(inputs, None)
    }
    /// Retains a caller-acquired production allowance through final access.
    /// The permit releases on rejection, abort or accepted settlement only after
    /// all access guards end, independently of observer lifetime and outcome claim.
    /// Provider-slot admission remains a separate caller-owned policy domain.
    pub fn prepare_with_permit<I>(
        inputs: I,
        permit: SCAdmissionPermit,
    ) -> (Self, SCSubmission<I, O, E>) {
        Self::prepare_owned(inputs, Some(permit))
    }
    fn prepare_owned<I>(
        inputs: I,
        permit: Option<SCAdmissionPermit>,
    ) -> (Self, SCSubmission<I, O, E>) {
        let state = Arc::new(Mutex::new(State {
            phase: SCProductionPhase::Prepared,
            active_accesses: 0,
            outcome: None,
            completion_recorded: false,
            outcome_claimed: false,
            permit,
        }));
        let production = Self {
            state: Arc::clone(&state),
        };
        let submission = SCSubmission {
            inputs: Some(inputs),
            settlement: SettlementGuard {
                state,
                settled: false,
            },
        };
        (production, submission)
    }
    /// Observes readiness separately from acceptance and final producer access.
    pub fn snapshot(&self) -> SCProductionSnapshot {
        let state = lock(&self.state);
        SCProductionSnapshot {
            phase: state.phase,
            active_accesses: state.active_accesses,
            completion_recorded: state.completion_recorded,
            outcome_claimed: state.outcome_claimed,
        }
    }
    /// Moves an accepted outcome only after every producer/access capability ends.
    /// Completion alone cannot make an output claimable; an accepted producer
    /// that ends without completing reports abandonment after all children end.
    pub fn claim(&self) -> SCProductionClaim<O, E> {
        let mut state = lock(&self.state);
        match state.phase {
            SCProductionPhase::Prepared => return SCProductionClaim::Pending,
            SCProductionPhase::Rejected => return SCProductionClaim::Rejected,
            SCProductionPhase::Aborted => return SCProductionClaim::Aborted,
            SCProductionPhase::Accepted => {}
        }
        if state.active_accesses != 0 {
            return SCProductionClaim::Pending;
        }
        if state.outcome_claimed {
            return SCProductionClaim::Taken;
        }
        state.outcome_claimed = true;
        match state.outcome.take() {
            Some(outcome) => SCProductionClaim::Ready(outcome),
            None => SCProductionClaim::Abandoned,
        }
    }
}

struct SettlementGuard<O, E> {
    state: Arc<Mutex<State<O, E>>>,
    settled: bool,
}
impl<O, E> Drop for SettlementGuard<O, E> {
    fn drop(&mut self) {
        if !self.settled {
            let (outcome, permit) = {
                let mut state = lock(&self.state);
                state.phase = SCProductionPhase::Aborted;
                (state.outcome.take(), state.settled_permit())
            };
            // User outcome cleanup is never called under the bookkeeping guard.
            drop(outcome);
            drop(permit);
        }
    }
}

/// Caller-owned inputs awaiting an explicit provider decision.
/// Drop aborts the unsubmitted attempt without starting or waiting for work.
pub struct SCSubmission<I, O, E> {
    // Field order settles unsubmitted input before its bookkeeping obligation.
    inputs: Option<I>,
    settlement: SettlementGuard<O, E>,
}
impl<I, O, E> SCSubmission<I, O, E> {
    /// Invokes a provider on the caller, outside all bookkeeping guards.
    /// Inline completion is staged until the provider returns `Accepted`.
    /// Rejection must return this attempt's root producer; a foreign producer is
    /// preserved as `InvalidRejection`, and the original attempt is aborted.
    /// Provider unwinding aborts submission without revoking escaped access guards.
    pub fn submit(
        mut self,
        provider: impl FnOnce(SCProducer<I, O, E>) -> SCProviderDecision<I, O, E>,
    ) -> SCSubmissionOutcome<I, O, E> {
        let state = Arc::clone(&self.settlement.state);
        lock(&state).active_accesses = 1;
        let producer = SCProducer {
            inputs: self.inputs.take(),
            lease: AccessLease {
                state: Arc::clone(&state),
            },
        };
        match provider(producer) {
            SCProviderDecision::Accepted => {
                let permit = {
                    let mut state = lock(&state);
                    state.phase = SCProductionPhase::Accepted;
                    state.settled_permit()
                };
                self.settlement.settled = true;
                drop(permit);
                SCSubmissionOutcome::Accepted
            }
            SCProviderDecision::Rejected(mut producer) => {
                if !Arc::ptr_eq(&state, &producer.lease.state) {
                    // SettlementGuard marks the original attempt aborted on return.
                    return SCSubmissionOutcome::InvalidRejection(producer);
                }
                let staged_outcome = {
                    let mut state = lock(&state);
                    state.phase = SCProductionPhase::Rejected;
                    state.outcome.take()
                };
                self.settlement.settled = true;
                let inputs = producer.inputs.take();
                drop(producer);
                SCSubmissionOutcome::Rejected(SCRejectedSubmission {
                    inputs,
                    staged_outcome,
                    production: SCProduction { state },
                })
            }
        }
    }
}

/// Provider decision made after invocation, distinct from a produced outcome.
pub enum SCProviderDecision<I, O, E> {
    Accepted,
    Rejected(SCProducer<I, O, E>),
}

/// Submission settlement with owned rejected or foreign capabilities preserved.
pub enum SCSubmissionOutcome<I, O, E> {
    Accepted,
    Rejected(SCRejectedSubmission<I, O, E>),
    InvalidRejection(SCProducer<I, O, E>),
}

/// Rejected unconsumed input and any outcome staged by an inline callback.
/// Consumed inputs remain in independent access guards; rejection does not wait
/// for them or revoke their ownership. Observation reports outstanding guards.
pub struct SCRejectedSubmission<I, O, E> {
    inputs: Option<I>,
    staged_outcome: Option<Result<O, E>>,
    production: SCProduction<O, E>,
}
impl<I, O, E> SCRejectedSubmission<I, O, E> {
    pub fn production(&self) -> SCProduction<O, E> {
        self.production.clone()
    }
    /// Returns caller-owned unconsumed input and staged outcome without cleanup.
    pub fn into_parts(self) -> (Option<I>, Option<Result<O, E>>) {
        (self.inputs, self.staged_outcome)
    }
}

/// Failure to extend an attempt's accepted access obligations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SCProductionError {
    AccessOverflow,
    Settled,
}

/// Rejected child discovery preserving its original inputs.
pub struct SCChildRejected<I> {
    inputs: I,
    reason: SCProductionError,
}
impl<I> SCChildRejected<I> {
    pub fn reason(&self) -> SCProductionError {
        self.reason
    }
    pub fn into_inputs(self) -> I {
        self.inputs
    }
}
impl<I> fmt::Debug for SCChildRejected<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SCChildRejected")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

/// Why a completion returned its original owned outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SCCompletionRejectionReason {
    AlreadyCompleted,
    Settled,
}

/// Completion rejection that neither drops nor copies the supplied outcome.
pub struct SCCompletionRejected<O, E> {
    outcome: Result<O, E>,
    reason: SCCompletionRejectionReason,
}
impl<O, E> SCCompletionRejected<O, E> {
    pub fn reason(&self) -> SCCompletionRejectionReason {
        self.reason
    }
    pub fn into_outcome(self) -> Result<O, E> {
        self.outcome
    }
}
impl<O, E> fmt::Debug for SCCompletionRejected<O, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SCCompletionRejected")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

struct AccessLease<O, E> {
    state: Arc<Mutex<State<O, E>>>,
}
impl<O, E> AccessLease<O, E> {
    fn extend(&self) -> Result<Self, SCProductionError> {
        let mut state = lock(&self.state);
        if matches!(
            state.phase,
            SCProductionPhase::Rejected | SCProductionPhase::Aborted
        ) {
            return Err(SCProductionError::Settled);
        }
        let next = state
            .active_accesses
            .checked_add(1)
            .ok_or(SCProductionError::AccessOverflow)?;
        state.active_accesses = next;
        Ok(Self {
            state: Arc::clone(&self.state),
        })
    }
    fn child<I>(&self, inputs: I) -> Result<SCProductionAccess<I, O, E>, SCChildRejected<I>> {
        match self.extend() {
            Ok(lease) => Ok(SCProductionAccess {
                inputs: Some(inputs),
                lease,
            }),
            Err(reason) => Err(SCChildRejected { inputs, reason }),
        }
    }
}
impl<O, E> Drop for AccessLease<O, E> {
    fn drop(&mut self) {
        let permit = {
            let mut state = lock(&self.state);
            state.active_accesses -= 1;
            state.settled_permit()
        };
        drop(permit);
    }
}

/// Non-cloneable root completion authority retaining input through actual access.
/// Completing records readiness but does not release input or this capability.
/// Retain it or a child access through actual downstream/provider final use;
/// callback return and SC capability release are not external quiescence receipts.
pub struct SCProducer<I, O, E> {
    // Input destruction (including panic unwinding) precedes lease release.
    inputs: Option<I>,
    lease: AccessLease<O, E>,
}
impl<I, O, E> SCProducer<I, O, E> {
    pub fn inputs(&self) -> Option<&I> {
        self.inputs.as_ref()
    }
    pub fn inputs_mut(&mut self) -> Option<&mut I> {
        self.inputs.as_mut()
    }
    /// Transfers consumed input to an independently counted final-access owner.
    /// Overflow or settlement preserves the input in this producer.
    pub fn consume_inputs(
        &mut self,
    ) -> Result<Option<SCProductionAccess<I, O, E>>, SCProductionError> {
        if self.inputs.is_none() {
            return Ok(None);
        }
        let lease = self.lease.extend()?;
        Ok(Some(SCProductionAccess {
            inputs: self.inputs.take(),
            lease,
        }))
    }
    /// Owns discovered child input or continuation state before handing it onward.
    /// Child access cannot complete the root attempt. Unit input is a continuation.
    pub fn child<J>(&self, inputs: J) -> Result<SCProductionAccess<J, O, E>, SCChildRejected<J>> {
        self.lease.child(inputs)
    }
    /// Stages success or accepted failure once, without ending producer access.
    /// Late and duplicate completion return their owned outcome to the caller.
    pub fn complete(&mut self, outcome: Result<O, E>) -> Result<(), SCCompletionRejected<O, E>> {
        let reason = {
            let mut state = lock(&self.lease.state);
            if matches!(
                state.phase,
                SCProductionPhase::Rejected | SCProductionPhase::Aborted
            ) {
                Some(SCCompletionRejectionReason::Settled)
            } else if state.completion_recorded {
                Some(SCCompletionRejectionReason::AlreadyCompleted)
            } else {
                state.completion_recorded = true;
                state.outcome = Some(outcome);
                return Ok(());
            }
        };
        Err(SCCompletionRejected {
            outcome,
            reason: reason.expect("rejected branch records reason"),
        })
    }
}

/// Non-cloneable ownership of consumed input, child input or continuation state.
/// This extends final-access settlement but carries no root completion authority.
/// Dropping consumer interest does not drop or cancel these guards. Explicitly
/// retain guards through the actual external last access; no scheduling is inferred.
pub struct SCProductionAccess<I, O, E> {
    inputs: Option<I>,
    lease: AccessLease<O, E>,
}
impl<I, O, E> SCProductionAccess<I, O, E> {
    pub fn inputs(&self) -> Option<&I> {
        self.inputs.as_ref()
    }
    pub fn inputs_mut(&mut self) -> Option<&mut I> {
        self.inputs.as_mut()
    }
    /// Retains nested discovery or continuation input before handoff.
    pub fn child<J>(&self, inputs: J) -> Result<SCProductionAccess<J, O, E>, SCChildRejected<J>> {
        self.lease.child(inputs)
    }
}
