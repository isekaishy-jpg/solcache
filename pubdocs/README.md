# Solcache

Solcache is a Rust library for reusable resource caching and publication
services. Implemented facilities include keyed storage, retained payloads, bounded
idle collection, shared production, consumer demand, declared allocation accounting
and independent admission policy. Versioned publication and declared dependency
invalidation, exclusive reusable pools, copied range storage and consumable
source buffers are implemented.
The library uses only `std`.

Its responsibility is to coordinate resource identity, shared production,
consumer interest, reusable results, residency, invalidation, and cache publication.
Different resource families may need different retention and readiness policies.
The application selects those contracts explicitly for each family.

CPU scheduling belongs to Solworker. File/network providers, format decoders,
rendering/device operations, and the application's event loop remain distinct
integration boundaries. Publishing a cache result does not by itself establish
scene readiness, GPU readiness, or permission to recycle externally accessed memory.

Public Solcache types and standalone functions use the `SC` prefix. Methods
and modules follow ordinary Rust naming. The crate remains
reusable across applications; application-specific identities and semantics must
be expressed through deliberate integration boundaries.

## Usage guide

The base case is Forever-informed mechanisms applied to Stock 3.3.5a workloads,
with Solworker supplying CPU execution. The APIs remain application independent:
use them directly, without an adapter layer or a Solworker dependency in Solcache.

| Read | Purpose |
| --- | --- |
| [Architecture and facility selection](architecture.md) | Decide where state belongs and which SC facility to use. |
| [Stock workload recipes](workloads.md) | Apply the mechanisms to all 22 mapped resource and consumer families. |
| [Direct Solworker integration](solworker.md) | Connect ownership, admission, demand, discovery, publication and shutdown. |
| [Family policies and accounting](policies.md) | Apply Forever budgets with their actual units and release boundaries. |
| [Research basis and limits](evidence.md) | Separate recovered behavior, Stock-supported extrapolation and implemented contracts. |

The reference sections below describe the implemented API. Generate its Rust
reference with `cargo doc --no-deps`. Start with the
[runnable examples](architecture.md#examples) for actual calls, including the
separate package that exercises both libraries. The guides describe intended
application compositions; they do not claim a running native client integration,
complete workload validation or measured performance gains.

## Ownership foundation

`SCBacking::try_new` acquires immutable shared backing and one declared allocation
charge. Moves and clones preserve the payload; clones add no allocation charge.
`SCView` borrows the payload or a projected part for the owner's borrow lifetime.
Acquisition overflow returns the original value in `SCAcquisitionError`.

Direct backing destroys its payload on the final dropping thread. A
`SCCleanupContext` instead queues final releases for its creating thread to drain.
The context is neither `Send` nor `Sync`; its owners can cross scoped threads but
cannot escape its lifetime. Context destruction drains pending values. Payload
destructors run outside bookkeeping locks, before their allocation charges end.
Callers must retain owners until any external device or provider finishes use.

`SCAccountingDomain` reports caller-declared bytes in exclusive temporary,
resident, retiring and idle-capacity classes. These are not measured process
memory: caller costs need to describe the backing being retained. A bare
`SCAllocationCharge` can transition class or transfer between domains; failure
returns the original charge. `SCBacking::from_charged` and
`SCCleanupContext::acquire_charged` bind an existing charge without releasing and
reacquiring it. `try_transition` changes class only with exclusive ownership;
shared owners cannot change class or domain. Final release enters retirement.

`SCPolicyCounter`, `SCCountLimit` and `SCByteLimit` keep family policy separate
from allocation accounting. Comparisons do not reserve resources. Byte thresholds
make equality and disable semantics explicit. The independent `SC_REFERENCE_*`
settings supply opt-in production (16), payload retention (32 MiB), provider-slot
(8) and range-entry (16) baselines; they impose no global limit or worker count.

Acquisition allocates shared-owner metadata. Accounting and cleanup bookkeeping
use synchronous mutexes; final deferred release can allocate queue capacity.
Operations start no threads or tasks. The caller drives cleanup and serializes
policy decisions with the state they govern. Payload interior mutability remains
subject to its own synchronization contract.

Run the [standalone ownership example](../examples/foundation.rs) with
`cargo run --example foundation`. Run `cargo test` for ownership, accounting and
policy contract tests. No executor, game types or adapter are required.

## Keyed storage and retention

`SCCache<K, T, E>` owns one typed key namespace. Include representation and source
scope in your key, or use separate cache instances where those meanings differ.
Keys use stable semantic `Hash` and `Eq`; a hash collision never establishes
identity. `SCIdentity<K>` remains valid through payload eviction and replacement,
but removal/recreation creates a new identity even if a storage slot is reused.
An identity handle alone does not retain a payload or the table.

`ensure` creates logical membership. `lookup` borrows a ready payload, `share`
clones its backing owner, and `take` moves the table's owner while leaving vacant
membership. `SCLookup` distinguishes absent, vacant, failed and ready outcomes.
Missing lookups start no work or consumer demand. A failed entry is not a ready
hit. A taken owner can still have other shared owners; taking does not promise
exclusive access to T.

`install` accepts backing in the Resident allocation class and an explicit
per-entry policy cost. Its result returns the stable identity and displaced owned
state. Overflow or an invalid class returns the supplied key, backing and policy
cost without changing membership. `fail` records a caller-supplied error;
`clear_payload` detaches state while preserving identity; `remove` also removes
membership. Displaced owners remain the caller's cleanup responsibility.

Mutations require `&mut SCCache`; borrowed views prevent removal, replacement and
collection for their lifetime. Shared/taken owners survive these operations and
table destruction. The table has no internal mutex; applications choose their
own synchronization or owner phase. User key operations and direct destructors
run on the calling thread. Wrapping a cache in an application lock also wraps any
user code those operations invoke, so arrange such calling contexts explicitly.

`maintain(limit, max_slots, force)` collects only payloads solely owned by their
cache slot. Sharing revives and pins an idle payload; releasing the final external
owner makes it eligible again. Forced collection also respects pins. Collection
keeps identity, removes the policy cost, and releases backing through its cleanup
contract. Deferred payloads remain accounted as retiring until their context drains.
Use `SC_REFERENCE_PAYLOAD_RETENTION` for the opt-in 32 MiB reference collection
target; other families can explicitly select their own `SCByteLimit` comparison.

Work is bounded by examined physical slots, including holes and failed/pinned
entries, with a cursor that resumes later. `SCRetentionReport` reports visits,
evictions, pins encountered and remaining byte pressure. These are observations
of that pass, not a global pin census or a wall-clock bound on user destructors.
Slots retain peak membership storage; removal makes slots reusable. If the same
backing occupies multiple slots, those owners conservatively pin one another.
Each slot has an independent policy cost; its backing still has only one physical
charge. Explicit removal/take can release such aliases.

Run [the keyed cache example](../examples/keyed_cache.rs) with
`cargo run --example keyed_cache` to see pressure, pin release, identity survival
and deferred cleanup together.

## Production and consumer demand

`SCDemand` owns independent interests through nonclone `SCDemandHandle` values.
Detaching one consumer preserves all others and never cancels producer access.
Updates carry caller-supplied per-consumer revisions: only a strictly newer
revision applies, including urgency decreases. Larger urgency values take priority.
Snapshots report maximum urgency and consumer count; `is_current` detects changes
at observation time, without serializing a later executor priority update.

`SCProductionMap` associates demand and shared attempts with `SCIdentity` tokens.
`interest` creates demand without work. `begin_shared` joins pending or unclaimed
completed work and returns unused input to its caller, or explicitly starts a new
attempt. Rejection, abort, claimed outcomes and settled abandonment permit retry;
rejection does not erase demand. `begin_candidate` starts independent owned work
for domains that permit multiple candidates. Pair each owned outcome with a
publication ticket to let `SCPublication` validate and select a winner.
Association removal preserves escaped access owners and existing consumers.

`SCProduction::prepare` creates observation and input ownership before invoking a
provider. `SCSubmission::submit` invokes the provider synchronously, outside
bookkeeping locks. The provider must return actual acceptance or return the same
root producer on rejection. Inline `complete` stages success or failure; a result
can be claimed only after acceptance and release of every producer/access guard.
Accepted work ending without completion reports abandonment. Provider unwinding
aborts the attempt without revoking escaped guards. Duplicate/late completion
returns the supplied outcome. Invalid rejection preserves the foreign producer
and aborts the original attempt.

`SCProducer::consume_inputs` moves input into a separately counted access guard;
rejection returns only unconsumed input and any staged outcome. Consumed guards
remain owned and observable through rejection. Child guards can retain discovered
inputs or continuation state, including nested discovery after the root ends;
they cannot complete the root. Hold each guard through actual external last use.
Mutable access to inputs does not prove device or provider quiescence. Consumer
detachment, callback return, completion and dropping a provider slot are distinct
from final access.

`SCAdmission` reserves checked counts through nonclone permits. Use independent
domains for production allowances and provider slots, with the opt-in reference
limits of 16 and 8. The map reserves production allowance; direct users can pass
a permit to `prepare_with_permit`. It releases after submission settlement and
final access, independently of result claim or observer lifetime. Provider-slot
permits follow the caller's provider operation. Neither domain counts retained
bytes or executor workers. Lowering a limit preserves existing reservations.

These APIs allocate shared bookkeeping and synchronously lock primitive state;
demand snapshots scan consumer table capacity. User providers and destructors run
outside internal locks. The map requires owner-side serialization. Joined
observers do not reserve an outcome; coordinate claim and cache application in
the owning caller. Association alone proves neither current cache membership nor
publication validity. No work is scheduled, retried or canceled implicitly.

Run [the production example](../examples/shared_production.rs) with
`cargo run --example shared_production` to see inline completion, consumed input,
and independent production/provider admission.

## Publication and declared dependencies

`SCPublication<K, T, E>` owns a keyed cache and its publication authority. `ensure`
creates membership; `begin_attempt` attaches a fresh publication round and captures
a supplied dependency snapshot without erasing the existing payload. A new round
revokes previous attempt tickets. Explicitly clone a ticket when the domain permits
independent candidates to compete for one winner. Tickets authorize publication,
not execution, and must remain paired with their corresponding outcomes.

When combining shared production and publication, serialize this request sequence
in the owning caller: check for a valid cached result first; otherwise call
`begin_shared`. On `Started`, create one `begin_attempt` ticket before submitting
the provider and keep it paired with that production through claim/publication.
On `Joined`, retain the existing production and return or release unused input;
do not call `begin_attempt`, because that would revoke the original ticket.
Joined observers do not each own a result; one coordinator claims and publishes,
and consumers share the installed backing. Serialize claim through publication
so another request cannot start duplicate work in the gap.

The map coalesces by cache identity, without comparing dependency revisions or
input values. Invalidation requires an explicit decision about existing work;
starting a publication round alone does not replace the map's old production.
Settle the old attempt or remove its association and establish current consumer
interest before starting replacement work. Removing an association neither
cancels its old users nor transfers their demand handles to the new group.
An old outcome must retain its old ticket and captured dependencies: assigning a
new ticket to it defeats freshness checking. SC's independent APIs cannot verify
that arbitrary owned bytes were actually produced under a supplied ticket.

Call `publish` with the owned success or failure claimed from production after
final access. It checks membership, current authority, current round and declared
dependencies before installing. Validation and installation share exclusive access
to the coordinator and a borrowed dependency registry. The first successful
installation closes the round. Stale candidates and installation errors return
the exact owned result and policy cost in `SCPublicationRejected`. Rejection leaves
the current payload and pending attachment intact. Successful installation returns
the displaced `SCStoredPayload` for caller cleanup; escaped old readers remain valid.

Successful backing must be Resident. Invalid allocation class and policy overflow
preserve the prior state and retry eligibility. An accepted failure installs failed
membership with zero retained-payload policy cost; it is not a usable hit. Return
values preserve ownership, including failures, without calling user destructors
during the validity/installation commit.

`SCDependencies` is an independently usable, caller-serialized revision registry.
Register source, representation, instance, view and device declarations separately.
Capture the selected declarations with `snapshot`; `advance` validates the complete
selected set before changing it, deduplicating repeated handles. Fresh opaque
identity/revision tokens prevent numeric wrap and removed-handle resurrection.
Even empty snapshots belong to their originating registry.

Propagation is explicit: the domain enumerates affected declarations. Captured
snapshots containing an advanced or removed declaration become invalid, while
unrelated scopes remain current. There is no inferred dependency graph or domain
mutation discovery. Retain actual backing separately, and serialize domain changes
with revision notification. Registry lookups are linear; capturing/advancing sets
allocates bookkeeping. These operations provide no performance or time-bound claim.

`lookup` and `share` on the publication coordinator validate stored dependencies
before returning a ready value or retained failure. Stale, removed and foreign
dependencies are typed errors. Existing borrowed or owned readers keep their
backing; invalidation blocks future reuse without destroying those readers.
`maintain` forwards bounded collection with the same pin and budget rules as the
keyed cache. Eviction preserves membership and pending attachment; a vacant entry
has no retained payload validity to check. `into_cache` consumes the coordinator
and explicitly discards its validity checks.

Authoritative metadata uses a separate path. `clear` removes payload and pending
attempt attachment while preserving authority. `authoritative` can repopulate
that entry without an attempt ticket. Passing an `SCAuthority` captured from
`authority` checks its local context; `replace_authority` clears the entry and
revokes prior tagged deliveries. Passing `None` means the caller vouches for the
current transport authority. SC cannot identify old untagged wire messages: bind
delivery to real provider context or settle the old stream before switching.
A successful authoritative update also revokes the pending attempt; a rejected
update does not. Its dependency snapshot is borrowed: retry using the original
captured stamp instead of recapturing newer revisions for an old payload.
`remove` destroys membership, so slot reuse cannot revive old
tickets. No multi-resource atomic update or delivery executor is implied.

Run [the versioned publication example](../examples/versioned_publication.rs) with
`cargo run --example versioned_publication` for production, publication, source
invalidation and retained-reader ownership together.

## Reusable storage and source bytes

`SCPool` stores caller-supplied objects with explicit layout keys and allocation
charges. Compatible checkout returns a noncloneable `SCPoolLease`; borrowing it
prevents return while those borrows remain active. Idle charges enter retained
capacity, checked-out charges enter temporary use, and discarded items enter
retirement through actual destruction. Nested owners keep their own charges.
Mutating valid length does not lower declared capacity; growing backing beyond
that declaration requires a newly charged item or separately charged nested owner.

Reset runs on the returning caller outside pool locks. `reset_and_return` returns
successfully reset storage; `return_to_pool` assumes the caller already reset it.
Dropping a lease, including during a failed reset, discards its storage. `trim`
bounds the number of idle items extracted, not destructor time. `close` and pool
destruction drain idle items; existing leases stay usable and later returns retire.
`with_cleanup` defers destruction to a borrowed `SCCleanupContext`, while reset
still runs on the returning caller. Closing never claims to join external users.

`SCSourceSnapshot` retains actual source context and an explicit source/revision
identity. A provider must keep that captured revision readable; SC cannot detect
unreported changes. `SCRangeStore` copies bytes under that snapshot and retains
one range per source identity. `copy_prefix` reports the actual copied count and
captures the same source for the remainder, including on a miss. Bounds are
checked before destination changes. `take` transfers a whole `SCRangeAllocation`
separately, retaining its capacity charge. Smaller replacement keeps capacity;
growth accounts for overlapping allocations before committing replacement.

Range occupancy is configurable; `reference` opts into sixteen entries with no
aggregate byte cap. Hits and changed inserts use a shared sequence; an identical
revision/offset/length insert increments only that entry's score without recopying
bytes. The first lowest score is replaced, and empty slots are reused first.
Identical insertion therefore requires interchangeable bytes within the declared
revision. Score exhaustion is a checked error. Snapshots distinguish valid bytes
from retained capacity and exclude the independently charged provider context.

`SCSourceBuffers` is a separate table for whole-source ownership transfer. IDs
must refer to one caller-defined freshness scope. Each successful nonzero read
adds its read size, even on a duplicate; the first owner remains stored and the
incoming duplicate owner is returned for caller cleanup. That incoming allocation
remains charged until settled. `take` subtracts the stored read size and transfers
the table's owner, preserving its full declared capacity and any earlier clones.
The supplied soft limit is checked after admission: equality allows another read,
strict excess stops the builder scope, and zero disables comparison. Taking does
not restart a stopped builder. Close resets policy state and releases table owners;
taken owners remain valid. The limit comes from the caller, with no OS memory query.

Run [the storage reuse example](../examples/reuse.rs) with
`cargo run --example reuse` for a copied prefix, a coherent remainder, scratch
reuse, and whole-source transfer using only SC and `std`.

## Direct execution and external use

The [manual execution example](../examples/manual_execution.rs) retains the
submission and its publication ticket in caller-owned storage, then invokes the
provider at an explicit service point. Cache lookup comes before production and
input preparation, so a warm request creates no job. The corresponding manual
queue test rejects a saturated enqueue without invoking or losing the submission,
then retries that same owned attempt after progress.

The [external lifetime example](../examples/external_lifetime.rs) models an
independent reader holding real byte backing after a production result is ready.
This is a simulated device contract, not a real graphics API integration. Immutable
ready data may be published while that reader retains an owner; the bytes and
their charge still cannot be destroyed until the reader releases it. When external
work must gate readiness instead, retain an `SCProductionAccess` through its last
access. Root callback return, production claim, external final use and owner-thread
destruction are distinct events. A cleanup context must be pumped after final
release; a completed job does not drain it automatically.

Run these standalone examples with `cargo run --example manual_execution` and
`cargo run --example external_lifetime`. Both depend only on SC and `std`.

The [direct Solworker example](../examples/solworker/README.md) is a separate
package requiring a sibling Solworker checkout. It exercises genuine saturated
admission, retained retry or abandonment, current demand forwarding, accepted
child discovery, owner-context publication, and transfer of an SW byte lease to
SC accounting. A rejected transfer retains the result and its original publication
ticket for an explicit retry. No adapter or SW dependency is added to SC itself.

Run it with `cargo run --locked --manifest-path examples/solworker/Cargo.toml`.
Its integration tests run separately with
`cargo test --locked --manifest-path examples/solworker/Cargo.toml`; the root
package's test command does not include this standalone package.

## Closure, maintenance and observation

`SCAdmission::close` permanently stops new reservations across every clone of
that count domain. Increasing its limit cannot reopen it. Rejection reports
`Closed` separately from capacity pressure and retains production input. Existing
permits remain owned; a prepared submission can still be submitted, shared callers
can join existing production, and accepted accesses can discover children. Those
operations extend already-admitted work rather than reopening root admission.

`SCAdmissionSnapshot::is_drained` means only that this domain is closed and its
permits have ended. Production-bound permits last through submission and final
SC access, but not through unclaimed results or escaped output ownership. A
provider-slot domain can drain while a separate external accessor is still live.
Every root entry point belonging to a host's close scope must use that scope's
admission gate; independent facilities do not inherit closure automatically.

Keep provider and owner services alive while stopping them can still deliver
callbacks or accepted discovery. Drive final access and delivery in their required
contexts before dismantling those services. A failed stop or wait retains the
provider state, its pending obligations and actual backing for later settlement.
Cache removal, pool/range/source closure and executor shutdown do not invalidate
escaped backing, checked-out leases or taken allocations.

`SCCleanupContext::drain_budget(maximum_records)` extracts at most that many
currently queued records. Zero performs no cleanup; unselected and newly queued
records remain owned. Selection order is unspecified. Destructors run outside
bookkeeping locks on the context thread, and charges remain live through them.
A destructor panic unwinds the selected batch while leaving unselected records
queued. Reentrant drains have their own budgets. The budget bounds selected
records, not arbitrary destructor time. Existing `drain` and context destruction
still process the whole current batch.

Observe each domain separately; there is no aggregate shutdown receipt:

| Observation | Meaning and limit |
| --- | --- |
| Admission snapshot | Closed gate, active permits and configured count limit. |
| Production snapshot | Acceptance, recorded outcome, active access capabilities and claim state; completion alone is not readiness. |
| Publication rejection | Original owned candidate plus the authority, attempt or dependency reason it could not be applied. |
| Pool snapshot | Closed state, idle storage and outstanding checkouts; excludes pending physical cleanup. |
| Retention report | Slots actually scanned, encountered pins and remaining policy pressure; does not free live owners. |
| Range/source snapshot | Storage retained by that facility, separate from taken allocations and source contexts. |
| Cleanup snapshot | Queued records and extracted destruction still in flight, including reentrant destruction; excludes live backing owners. |
| Accounting snapshot | Declared allocation costs through actual destruction; zero bytes can still leave zero-cost cleanup records. |

Cache maintenance budgets count physical slots, pool trim budgets count items,
and cleanup budgets count records. They are independent of family byte thresholds.
Existing all-at-once facility `close` calls are not budgeted maintenance. Snapshot
reads execute no callbacks and separate snapshots are not a coherent global view.

The [integrated shutdown tests](../tests/shutdown.rs) use real bytes with a
simulated callback-producing provider, an actual timed-out wait, and escaped
facility owners. The [SW shutdown test](../examples/solworker/tests/shutdown.rs)
keeps actual owner delivery service alive after runtime root closure and verifies
that a joined executor does not retire its escaped SC output.
