//! Caller-driven collection with a bounded, persistent physical-slot scan.

use super::{SCCache, SCStoredPayload};
use crate::policy::SCByteLimit;

/// The result of one synchronous, bounded retention service call.
///
/// Policy bytes count entry contributions, independently of declared allocation
/// bytes. Pinned counts describe only this pass, not the entire cache. Observing
/// this result never drives deferred cleanup or certifies physical retirement.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SCRetentionReport {
    /// Physical slots examined, including vacant holes left by removed entries.
    pub scanned_slots: usize,
    /// Logical entries encountered, including unavailable and pinned entries.
    pub visited_entries: usize,
    /// Eligible resident payloads detached; their logical identities survive.
    pub evicted_payloads: usize,
    /// Per-entry policy contributions removed before final backing release.
    pub removed_policy_bytes: u64,
    /// Resident payloads encountered with additional backing owners.
    pub pinned_encountered: usize,
    /// Remaining cache policy bytes after this pass.
    pub policy_bytes_after: u64,
    /// Whether the configured threshold remains reached after this pass.
    pub pressure_after: bool,
    /// Whether this call examined every physical slot; true for empty storage.
    pub completed_cycle: bool,
    /// Persistent index where the next scan resumes, zero for empty storage.
    pub next_slot: usize,
    /// Physical scan extent, including holes; distinct from logical entry count.
    pub slot_count: usize,
}

impl<K, T, E> SCCache<'_, K, T, E> {
    /// Collects idle payloads while the selected family threshold is reached.
    ///
    /// Idle means the cache holds the sole backing owner. Sharing a payload pins
    /// it immediately; dropping the final external owner makes it eligible again.
    /// Logical identity handles do not pin payloads. The same backing installed in
    /// multiple entries is conservatively pinned until only one owner remains.
    ///
    /// At most `max_slots` physical slots and one complete storage cycle are
    /// examined, including holes, unavailable entries and pinned payloads. The
    /// persistent cursor prevents repeated prefixes from starving later slots.
    /// `force` collects eligible payloads regardless of the byte threshold, while
    /// preserving live pins. Zero work performs no collection.
    ///
    /// Collection commits identity-preserving detachment and removes the entry's
    /// policy contribution before releasing backing. Final destruction follows
    /// the backing's cleanup context; queued retirement stays physically charged.
    /// Direct payload destructors run on this caller and may panic. A panic leaves
    /// the detachment, policy removal and cursor advance committed.
    pub fn maintain(
        &mut self,
        limit: SCByteLimit,
        max_slots: usize,
        force: bool,
    ) -> SCRetentionReport {
        let slot_count = self.storage.slots.len();
        let mut report = SCRetentionReport {
            slot_count,
            ..SCRetentionReport::default()
        };
        for _ in 0..max_slots.min(slot_count) {
            if !force && !limit.reached(self.policy_bytes) {
                break;
            }
            let index = self.storage.cursor;
            self.storage.cursor = if index + 1 == slot_count {
                0
            } else {
                index + 1
            };
            report.scanned_slots += 1;
            let Some(entry) = self.storage.slots[index].as_mut() else {
                continue;
            };
            report.visited_entries += 1;
            let SCStoredPayload::Ready(backing) = &entry.state else {
                continue;
            };
            if !backing.is_unique() {
                report.pinned_encountered += 1;
                continue;
            }
            let removed = std::mem::replace(&mut entry.state, SCStoredPayload::Vacant);
            let policy_bytes = std::mem::take(&mut entry.policy_bytes);
            self.policy_bytes -= policy_bytes;
            report.evicted_payloads += 1;
            report.removed_policy_bytes += policy_bytes;
            // No cache bookkeeping or borrowed entry survives user destruction.
            drop(removed);
        }
        report.policy_bytes_after = self.policy_bytes;
        report.pressure_after = limit.reached(self.policy_bytes);
        report.completed_cycle = report.scanned_slots == slot_count;
        report.next_slot = self.storage.cursor;
        report
    }
}
