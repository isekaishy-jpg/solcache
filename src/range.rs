//! Copied prefix-range reuse within explicitly retained source revisions.

use std::fmt;
use std::sync::Arc;

use crate::accounting::{
    SCAccountingDomain, SCAccountingError, SCAllocationCharge, SCAllocationClass,
};
use crate::ownership::{SCBacking, SCView};
use crate::policy::{SC_REFERENCE_RANGE_ENTRIES, SCCountLimit};

struct Token;

/// A caller-declared source revision retaining the actual provider/source context.
///
/// Clones name the same source and revision and retain its backing. The provider
/// must keep the context suitable for reading this revision through final use,
/// including remainder reads. Interior mutation, key equality and a live provider
/// pointer do not prove content freshness. SC performs no I/O and cannot discover
/// an unreported source change.
pub struct SCSourceSnapshot<'cleanup, T> {
    identity: Arc<Token>,
    revision: Arc<Token>,
    context: SCBacking<'cleanup, T>,
}

impl<'cleanup, T> SCSourceSnapshot<'cleanup, T> {
    /// Starts a fresh source identity and revision, consuming retained backing.
    pub fn new(context: SCBacking<'cleanup, T>) -> Self {
        Self {
            identity: Arc::new(Token),
            revision: Arc::new(Token),
            context,
        }
    }

    /// Declares a fresh revision of this source with its retained read context.
    /// Old snapshots retain their captured revision under the provider contract.
    /// Tokens never wrap or recycle while a captured snapshot exists. This is a
    /// caller freshness assertion, not validation of an authoritative latest
    /// revision; old revision admission remains the caller's responsibility.
    pub fn revised(&self, context: SCBacking<'cleanup, T>) -> Self {
        Self {
            identity: Arc::clone(&self.identity),
            revision: Arc::new(Token),
            context,
        }
    }

    pub fn context(&self) -> SCView<'_, T> {
        self.context.view()
    }

    pub fn same_source(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.identity, &other.identity)
    }

    pub fn same_revision(&self, other: &Self) -> bool {
        self.same_source(other) && Arc::ptr_eq(&self.revision, &other.revision)
    }
}

impl<T> Clone for SCSourceSnapshot<'_, T> {
    fn clone(&self) -> Self {
        Self {
            identity: Arc::clone(&self.identity),
            revision: Arc::clone(&self.revision),
            context: self.context.clone(),
        }
    }
}

/// A synchronous operation failed before changing bytes or membership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SCRangeError {
    Closed,
    Disabled,
    BoundsOverflow,
    AllocationFailed,
    Accounting(SCAccountingError),
    ScoreExhausted,
}

impl fmt::Display for SCRangeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => formatter.write_str("range store is closed"),
            Self::Disabled => formatter.write_str("range store occupancy is disabled"),
            Self::BoundsOverflow => formatter.write_str("range offset and length overflow u64"),
            Self::AllocationFailed => formatter.write_str("range allocation failed"),
            Self::Accounting(error) => error.fmt(formatter),
            Self::ScoreExhausted => formatter.write_str("range replacement score exhausted"),
        }
    }
}

impl std::error::Error for SCRangeError {}

/// Copied insertion outcome, independent of physical bytes retained.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SCRangeInsert {
    Inserted,
    Replaced,
    Refreshed,
}

/// Actual copied prefix and retained source context for the remaining read.
///
/// A miss copies zero bytes and still captures the requested snapshot. This result
/// borrows neither store membership nor destination memory. A remainder provider
/// call must use `source()` and `remaining_offset()`, not recapture current source.
pub struct SCRangeRead<'cleanup, T> {
    source: SCSourceSnapshot<'cleanup, T>,
    copied: usize,
    remaining_offset: u64,
    remaining_length: usize,
}

impl<'cleanup, T> SCRangeRead<'cleanup, T> {
    pub fn source(&self) -> &SCSourceSnapshot<'cleanup, T> {
        &self.source
    }
    pub fn copied(&self) -> usize {
        self.copied
    }
    pub fn remaining_offset(&self) -> u64 {
        self.remaining_offset
    }
    pub fn remaining_length(&self) -> usize {
        self.remaining_length
    }
    pub fn is_complete(&self) -> bool {
        self.remaining_length == 0
    }
}

/// Entire removed range allocation with source context and capacity charge.
///
/// Taking transfers ownership without copying or releasing its charge. Valid
/// bytes may occupy less than retained capacity. Drop releases the allocation
/// before its charge; the source context follows its own cleanup rule.
pub struct SCRangeAllocation<'cleanup, T> {
    source: SCSourceSnapshot<'cleanup, T>,
    offset: u64,
    storage: ChargedBytes,
}

struct ChargedBytes {
    bytes: Vec<u8>,
    charge: Option<SCAllocationCharge>,
}

impl ChargedBytes {
    fn charge(&self) -> &SCAllocationCharge {
        self.charge
            .as_ref()
            .expect("live range storage retains its charge")
    }

    fn into_parts(mut self) -> (Vec<u8>, SCAllocationCharge) {
        let bytes = std::mem::take(&mut self.bytes);
        let charge = self
            .charge
            .take()
            .expect("live range storage retains its charge");
        (bytes, charge)
    }
}

impl Drop for ChargedBytes {
    fn drop(&mut self) {
        if let Some(charge) = &mut self.charge {
            charge.transition(SCAllocationClass::Retiring);
        }
        // Field order releases byte-vector storage before releasing its charge.
    }
}

impl<'cleanup, T> SCRangeAllocation<'cleanup, T> {
    pub fn source(&self) -> &SCSourceSnapshot<'cleanup, T> {
        &self.source
    }
    pub fn offset(&self) -> u64 {
        self.offset
    }
    pub fn bytes(&self) -> &[u8] {
        &self.storage.bytes
    }
    pub fn capacity(&self) -> usize {
        self.storage.bytes.capacity()
    }
    pub fn declared_bytes(&self) -> u64 {
        self.storage.charge().declared_bytes()
    }

    /// Transfers all ownership, including the non-cloneable charge. The caller
    /// must keep the charge until the allocation has actually been released.
    pub fn into_parts(
        self,
    ) -> (
        SCSourceSnapshot<'cleanup, T>,
        u64,
        Vec<u8>,
        SCAllocationCharge,
    ) {
        let (bytes, charge) = self.storage.into_parts();
        (self.source, self.offset, bytes, charge)
    }
}

struct Entry<'cleanup, T> {
    allocation: SCRangeAllocation<'cleanup, T>,
    score: u64,
}

/// Occupancy and copied costs, excluding separately retained source backing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SCRangeSnapshot {
    pub entries: usize,
    pub entry_limit: usize,
    pub valid_bytes: u64,
    pub retained_capacity_bytes: u64,
    pub closed: bool,
}

/// Independently usable, single-owner, caller-serialized copied range store.
///
/// One range is retained per source identity. Revisions gate reuse; updates
/// replace that identity's previous range. Hits and changed insertions use a
/// shared sequence; identical insertion increments only that entry's score
/// without comparing or recopying bytes. Eviction selects the first minimum
/// score, matching the recovered mixed rule rather than conventional LRU.
/// Identical insertion requires interchangeable bytes for a revision. Scores
/// fail explicitly on exhaustion rather than wrapping.
///
/// Occupancy bounds keys and linear scans, not aggregate bytes. Allocation,
/// copying and source-owner cloning are synchronous. No store lock, callback,
/// worker, provider read or hidden maintenance is involved. Byte-vector capacity
/// is charged once in the selected accounting domain.
pub struct SCRangeStore<'cleanup, T> {
    domain: SCAccountingDomain,
    entry_limit: usize,
    entries: Vec<Option<Entry<'cleanup, T>>>,
    sequence: u64,
    closed: bool,
}

impl<'cleanup, T> SCRangeStore<'cleanup, T> {
    /// Configures occupancy explicitly; zero disables copied range admission.
    pub fn new(entry_limit: SCCountLimit, domain: &SCAccountingDomain) -> Self {
        Self {
            domain: domain.clone(),
            entry_limit: entry_limit.limit(),
            entries: Vec::new(),
            sequence: 1,
            closed: false,
        }
    }

    /// Opts into the recovered sixteen-entry reference occupancy.
    pub fn reference(domain: &SCAccountingDomain) -> Self {
        Self::new(SC_REFERENCE_RANGE_ENTRIES, domain)
    }

    /// Copies input or refreshes an identical revision/range without copying.
    ///
    /// Expected errors preserve membership and caller input. Smaller replacements
    /// keep capacity and its charge. Growth temporarily owns both allocations;
    /// accounting overflow rejects before commit.
    pub fn insert(
        &mut self,
        source: &SCSourceSnapshot<'cleanup, T>,
        offset: u64,
        bytes: &[u8],
    ) -> Result<SCRangeInsert, SCRangeError> {
        self.check_open()?;
        checked_end(offset, bytes.len())?;
        if self.entry_limit == 0 {
            return Err(SCRangeError::Disabled);
        }
        let existing = self.entries.iter().position(|slot| {
            slot.as_ref()
                .is_some_and(|entry| entry.allocation.source.same_source(source))
        });
        if let Some(index) = existing {
            let entry = self.entries[index]
                .as_mut()
                .expect("matching slot contains an entry");
            if entry.allocation.source.same_revision(source)
                && entry.allocation.offset == offset
                && entry.allocation.storage.bytes.len() == bytes.len()
            {
                entry.score = entry
                    .score
                    .checked_add(1)
                    .ok_or(SCRangeError::ScoreExhausted)?;
                return Ok(SCRangeInsert::Refreshed);
            }
        }
        let next = self
            .sequence
            .checked_add(1)
            .ok_or(SCRangeError::ScoreExhausted)?;
        let index = existing
            .or_else(|| self.entries.iter().position(Option::is_none))
            .or_else(|| {
                (self.entries.len() == self.entry_limit).then(|| {
                    self.entries
                        .iter()
                        .enumerate()
                        .filter_map(|(index, slot)| slot.as_ref().map(|entry| (index, entry.score)))
                        .min_by_key(|(_, score)| *score)
                        .map(|(index, _)| index)
                        .expect("a full enabled range store has an entry")
                })
            });
        if let Some(index) = index {
            if let Some(entry) = self.entries[index]
                .as_mut()
                .filter(|entry| entry.allocation.storage.bytes.capacity() >= bytes.len())
            {
                let retained_source = source.clone();
                entry.allocation.storage.bytes.clear();
                entry.allocation.storage.bytes.extend_from_slice(bytes);
                let old_source = std::mem::replace(&mut entry.allocation.source, retained_source);
                entry.allocation.offset = offset;
                entry.score = self.sequence;
                self.sequence = next;
                drop(old_source);
                return Ok(SCRangeInsert::Replaced);
            }
        } else {
            self.entries
                .try_reserve(1)
                .map_err(|_| SCRangeError::AllocationFailed)?;
        }
        let allocation = self.copy_allocation(source, offset, bytes)?;
        let entry = Entry {
            allocation,
            score: self.sequence,
        };
        let (outcome, old_entry) = if let Some(index) = index {
            let old = self.entries[index].replace(entry);
            let outcome = if old.is_some() {
                SCRangeInsert::Replaced
            } else {
                SCRangeInsert::Inserted
            };
            (outcome, old)
        } else {
            self.entries.push(Some(entry));
            (SCRangeInsert::Inserted, None)
        };
        self.sequence = next;
        drop(old_entry);
        Ok(outcome)
    }

    /// Copies the covered beginning of a request and returns actual byte count.
    /// Bounds are checked even on misses; errors leave destination unchanged.
    /// A zero-length request can refresh a contained entry's score.
    pub fn copy_prefix(
        &mut self,
        source: &SCSourceSnapshot<'cleanup, T>,
        offset: u64,
        destination: &mut [u8],
    ) -> Result<SCRangeRead<'cleanup, T>, SCRangeError> {
        self.check_open()?;
        checked_end(offset, destination.len())?;
        let mut copied = 0;
        if let Some(entry) = self
            .entries
            .iter_mut()
            .filter_map(Option::as_mut)
            .find(|entry| entry.allocation.source.same_revision(source))
        {
            let allocation = &entry.allocation;
            let end = checked_end(allocation.offset, allocation.storage.bytes.len())?;
            if offset >= allocation.offset && offset < end {
                let next = self
                    .sequence
                    .checked_add(1)
                    .ok_or(SCRangeError::ScoreExhausted)?;
                let relative = usize::try_from(offset - allocation.offset)
                    .map_err(|_| SCRangeError::BoundsOverflow)?;
                copied = destination
                    .len()
                    .min(allocation.storage.bytes.len() - relative);
                destination[..copied]
                    .copy_from_slice(&allocation.storage.bytes[relative..relative + copied]);
                entry.score = self.sequence;
                self.sequence = next;
            }
        }
        Ok(SCRangeRead {
            source: source.clone(),
            copied,
            remaining_offset: checked_end(offset, copied)?,
            remaining_length: destination.len() - copied,
        })
    }

    /// Transfers the entire matching revision's range allocation separately
    /// from non-consuming copied prefix hits.
    pub fn take(
        &mut self,
        source: &SCSourceSnapshot<'cleanup, T>,
    ) -> Result<Option<SCRangeAllocation<'cleanup, T>>, SCRangeError> {
        self.check_open()?;
        let index = self.entries.iter().position(|slot| {
            slot.as_ref()
                .is_some_and(|entry| entry.allocation.source.same_revision(source))
        });
        Ok(index.and_then(|index| self.entries[index].take().map(|entry| entry.allocation)))
    }

    pub fn snapshot(&self) -> SCRangeSnapshot {
        // Capacity charges share one u64 domain; their subsets and valid lengths
        // therefore cannot overflow that domain's checked total.
        SCRangeSnapshot {
            entries: self.entries.iter().filter(|slot| slot.is_some()).count(),
            entry_limit: self.entry_limit,
            valid_bytes: self
                .entries
                .iter()
                .filter_map(Option::as_ref)
                .map(|entry| entry.allocation.storage.bytes.len() as u64)
                .sum(),
            retained_capacity_bytes: self
                .entries
                .iter()
                .filter_map(Option::as_ref)
                .map(|entry| entry.allocation.storage.charge().declared_bytes())
                .sum(),
            closed: self.closed,
        }
    }

    /// Closes admission and destroys store allocations synchronously. Taken
    /// allocations and read contexts remain valid. Repeated close is harmless;
    /// there are no outstanding store borrows to join.
    pub fn close(&mut self) -> usize {
        self.closed = true;
        let entries = std::mem::take(&mut self.entries);
        let count = entries.iter().filter(|slot| slot.is_some()).count();
        drop(entries);
        count
    }

    fn check_open(&self) -> Result<(), SCRangeError> {
        if self.closed {
            Err(SCRangeError::Closed)
        } else {
            Ok(())
        }
    }

    fn copy_allocation(
        &self,
        source: &SCSourceSnapshot<'cleanup, T>,
        offset: u64,
        input: &[u8],
    ) -> Result<SCRangeAllocation<'cleanup, T>, SCRangeError> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(input.len())
            .map_err(|_| SCRangeError::AllocationFailed)?;
        bytes.extend_from_slice(input);
        let capacity = u64::try_from(bytes.capacity()).map_err(|_| SCRangeError::BoundsOverflow)?;
        let charge = self
            .domain
            .charge(capacity, SCAllocationClass::Resident)
            .map_err(SCRangeError::Accounting)?;
        Ok(SCRangeAllocation {
            source: source.clone(),
            offset,
            storage: ChargedBytes {
                bytes,
                charge: Some(charge),
            },
        })
    }
}

fn checked_end(offset: u64, length: usize) -> Result<u64, SCRangeError> {
    let length = u64::try_from(length).map_err(|_| SCRangeError::BoundsOverflow)?;
    offset
        .checked_add(length)
        .ok_or(SCRangeError::BoundsOverflow)
}

#[cfg(test)]
#[path = "../tests/unit/range.rs"]
mod tests;
