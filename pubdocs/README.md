# Solcache

Solcache is a Rust library project for reusable resource caching and publication
services. The first implementation stage provides ownership, declared allocation
accounting and independent policy primitives. Keyed caching, shared production,
publication, pools and range storage remain planned. The library uses only `std`.

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
reacquiring it. Once bound, the acquisition class stays fixed until final release
transitions it to retiring. Shared backing has no class/domain mutation API yet.

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
