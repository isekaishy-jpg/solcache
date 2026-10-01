# Solcache

Solcache is a Rust library project for reusable resource caching and publication
services. Implemented facilities include keyed storage, retained payloads, bounded
idle collection, shared production, consumer demand, declared allocation accounting
and independent admission policy. Versioned publication and declared dependency
invalidation are implemented; pools and range storage remain planned.
The library uses only `std`.

The proposed responsibility is to coordinate resource identity, shared production,
consumer interest, reusable results, residency, invalidation, and cache publication.
Different resource families may need different retention and readiness policies.
The design must establish those contracts before selecting a common mechanism.

CPU scheduling belongs to Solworker. File/network providers, format decoders,
rendering/device operations, and the application's event loop remain distinct
integration boundaries. Publishing a cache result does not by itself establish
scene readiness, GPU readiness, or permission to recycle externally accessed memory.

Public Solcache types and standalone functions will use the `SC` prefix. Methods
and modules will follow ordinary Rust naming. The crate is intended to remain
reusable across applications; application-specific identities and semantics must
be expressed through deliberate integration boundaries.

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
