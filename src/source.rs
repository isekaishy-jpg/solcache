//! Consumable whole-source buffers and duplicate-sensitive read accounting.

use std::collections::BTreeMap;
use std::fmt;

use crate::accounting::SCAllocationClass;
use crate::ownership::SCBacking;

/// Why a completed source read was not admitted. Its owner is returned intact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SCSourceRejectionReason {
    Closed,
    SoftLimitReached,
    EmptyRead,
    InvalidClass,
    CounterOverflow,
}

/// Rejected input, including its independently charged backing.
pub struct SCSourceRejected<'cleanup, T> {
    backing: SCBacking<'cleanup, T>,
    reason: SCSourceRejectionReason,
}

impl<'cleanup, T> SCSourceRejected<'cleanup, T> {
    pub fn reason(&self) -> SCSourceRejectionReason {
        self.reason
    }

    pub fn into_parts(self) -> (SCBacking<'cleanup, T>, SCSourceRejectionReason) {
        (self.backing, self.reason)
    }
}

impl<T> fmt::Debug for SCSourceRejected<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SCSourceRejected")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

/// Result of recording a nonzero read. Both outcomes charge the read measure.
pub enum SCSourceInsert<'cleanup, T> {
    Stored,
    /// The first buffer remains stored; the incoming owner is returned for cleanup.
    Duplicate(SCBacking<'cleanup, T>),
}

impl<T> fmt::Debug for SCSourceInsert<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stored => f.write_str("Stored"),
            Self::Duplicate(_) => f.write_str("Duplicate(..)"),
        }
    }
}

/// Read-policy state, deliberately separate from physical allocation accounting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SCSourceBufferSnapshot {
    pub retained_entries: usize,
    pub read_bytes: u64,
    pub enabled: bool,
    pub may_read: bool,
    pub closed: bool,
}

/// A caller-serialized table transferring whole backing owners on hits.
///
/// IDs belong to one caller-defined source/freshness scope. Recreate or close the
/// table when that scope changes. No source I/O or OS memory query is performed.
/// The explicit soft limit is checked after recording each successful nonzero
/// read, including duplicates: equality permits another read, strict excess
/// stops this builder scope, and zero disables the comparison. The crossing read
/// is retained. Taking an entry does not restart a stopped builder.
///
/// `read_bytes` is a policy measure, not unique bytes or allocation capacity.
/// Callers supply Resident backing charged for its full declared capacity; a
/// smaller read length never changes that physical charge. Taking transfers the
/// table's owner without copying or promising uniqueness against earlier clones.
/// No internal locks, callbacks, worker threads or global memory cap are involved.
pub struct SCSourceBuffers<'cleanup, T> {
    entries: BTreeMap<u64, (SCBacking<'cleanup, T>, u64)>,
    soft_limit: u64,
    read_bytes: u64,
    enabled: bool,
    stopped: bool,
    closed: bool,
}

impl<'cleanup, T> SCSourceBuffers<'cleanup, T> {
    /// Starts a fresh source scope with a caller-supplied soft read limit.
    pub fn new(soft_limit: u64) -> Self {
        Self {
            entries: BTreeMap::new(),
            soft_limit,
            read_bytes: 0,
            enabled: false,
            stopped: false,
            closed: false,
        }
    }

    /// Whether the builder may begin another read. This is not a reservation.
    pub fn may_read(&self) -> bool {
        !self.closed && !self.stopped
    }

    /// Records an already completed read, retaining the first buffer for its ID.
    ///
    /// Check `may_read` before issuing the next read in this serialized scope.
    /// Errors preserve the supplied owner and all table state. Duplicate cleanup
    /// is explicit through the returned owner, after the counter is committed.
    pub fn insert_read(
        &mut self,
        id: u64,
        backing: SCBacking<'cleanup, T>,
        read_bytes: u64,
    ) -> Result<SCSourceInsert<'cleanup, T>, SCSourceRejected<'cleanup, T>> {
        let reason = if self.closed {
            Some(SCSourceRejectionReason::Closed)
        } else if self.stopped {
            Some(SCSourceRejectionReason::SoftLimitReached)
        } else if read_bytes == 0 {
            Some(SCSourceRejectionReason::EmptyRead)
        } else if backing.allocation_class() != SCAllocationClass::Resident {
            Some(SCSourceRejectionReason::InvalidClass)
        } else if self.read_bytes.checked_add(read_bytes).is_none() {
            Some(SCSourceRejectionReason::CounterOverflow)
        } else {
            None
        };
        if let Some(reason) = reason {
            return Err(SCSourceRejected { backing, reason });
        }
        self.read_bytes += read_bytes;
        self.enabled = true;
        self.stopped = self.soft_limit != 0 && self.read_bytes > self.soft_limit;
        match self.entries.entry(id) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert((backing, read_bytes));
                Ok(SCSourceInsert::Stored)
            }
            std::collections::btree_map::Entry::Occupied(_) => {
                Ok(SCSourceInsert::Duplicate(backing))
            }
        }
    }

    /// Removes one whole owner and subtracts its recorded read size only.
    /// Escaped owners retain their full allocation charge through final cleanup.
    pub fn take(&mut self, id: u64) -> Option<SCBacking<'cleanup, T>> {
        let (backing, bytes) = self.entries.remove(&id)?;
        self.read_bytes -= bytes;
        Some(backing)
    }

    pub fn snapshot(&self) -> SCSourceBufferSnapshot {
        SCSourceBufferSnapshot {
            retained_entries: self.entries.len(),
            read_bytes: self.read_bytes,
            enabled: self.enabled,
            may_read: self.may_read(),
            closed: self.closed,
        }
    }

    /// Disables this scope, resets its policy counter, and drops retained owners.
    /// State is committed before payload cleanup, including panicking cleanup.
    /// Taken owners remain valid; this does not join their users.
    pub fn close(&mut self) {
        self.closed = true;
        self.enabled = false;
        self.read_bytes = 0;
        let entries = std::mem::take(&mut self.entries);
        drop(entries);
    }
}
