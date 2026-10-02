# Architecture and facility selection

[Guide index](README.md)

Solcache (SC) owns reusable values and the rules for accepting them. Solworker
(SW) owns CPU execution and delivery. The application supplies resource meaning
and drives progress. This division comes from the Stock/Forever loading,
composition and reclamation paths summarized in the [research basis](evidence.md).

## Where each responsibility belongs

| Owner | Responsibilities | Typical handoff |
| --- | --- | --- |
| Application/resource provider | Semantic keys, representation requirements, current source, recipes, validity, retry and fallback decisions. | Owned inputs plus captured validity to an SC production. |
| SC | Identity, shared backing, pending-production association, independent interest, eligible publication, retention, pools, ranges and declared accounting. | Owned operation state to execution; an accepted value to consumers. |
| SW | CPU jobs, prerequisite edges, resource ordering, owned queue admission, discovery, owner delivery and execution shutdown. | Prepared output or typed failure to the application owner. |
| File/archive/network/audio provider | I/O, transport authority, provider slots, cancellation settlement and source final use. | Owned bytes or a result with an explicit consumption contract. |
| Renderer (SR) | Device namespace, uploads, frame resources, backend destruction and GPU completion. | Explicit final-use release of retained CPU/device input owners. |
| Host event loop | Provider service, bounded demand propagation, owner pumping, maintenance and cleanup. | Progress until the relevant obligations settle. |

These are responsibilities, not mandatory wrapper types or threads. A host can
use SC synchronously, with SW, or with another executor. SC starts no workers and
does not call a renderer, infer provider readiness or run an application loop.

## Keep the resource layers distinct

For a texture, distinguish the selected archive/file source, decoded CPU
representation, device representation and each scene consumer. A model similarly
has shared immutable data and separate placement/animation state. A source hit
is useful even when a device reset makes the GPU representation unusable.

Use typed keys or separate tables for different meanings. Include the relevant
representation in identity: format, quality or recipe can change compatibility
even when the asset name stays equal. Let the provider decide which dimensions
matter. Do not fabricate Forever-only channels for Stock content.

Register validity declarations in `SCDependencies` for source, representation,
instance, view and device as appropriate. Capture only the declarations the
product depends on. Advance the affected declarations with the domain change;
the registry does not discover those changes. An owned source snapshot preserves
the selected source's backing; a revision token alone does not own that backing.
See [source selection](evidence.md#sources) and
[representation changes](evidence.md#composition).

Old owners remain readable after invalidation or replacement. New lookup must
check current eligibility. Do not mutate shared immutable backing to make an old
reader appear to have observed a new source or a new device generation.

## Choose the smallest useful facility

| Need | Facility | Caller obligation |
| --- | --- | --- |
| Own immutable bytes or an opaque value across users | `SCBacking`, borrowed `SCView` | Declare the allocation cost; retain through every actual accessor. |
| Defer destruction to a known thread | `SCCleanupContext` | Keep that thread/context alive and drain it. Scoped owners cannot escape its lifetime. |
| Store/share/take a keyed result | `SCCache` | Supply semantic equality and serialize mutation. Identity alone does not pin bytes. |
| Reject outdated results or future reuse | `SCPublication`, `SCDependencies` | Pair outcomes with original tickets and declared dependencies. |
| Coalesce in-flight work for one identity | `SCProductionMap::begin_shared` | Serialize lookup, start, claim and application; explicitly supersede stale associations. |
| Own one operation without a shared cache | `SCProduction::prepare` | Return actual provider acceptance/rejection and retain input accesses. |
| Track consumers and family concurrency | `SCDemand`, `SCAdmission` | Forward current demand and select separate admission domains. |
| Reuse exclusive scratch/output capacity | `SCPool` | Define compatible layout, minimum capacity and safe reset/return. |
| Copy a cached prefix of source bytes | `SCRangeStore`, `SCSourceSnapshot` | Preserve source coherence for the rest of the read; distinguish valid length from capacity. |
| Retain then consume a whole source buffer | `SCSourceBuffers` | Select scope, read-size policy and caller-supplied soft limit. |
| Account for owned memory and policy pressure | `SCAllocationCharge`, policy helpers | Keep physical ownership and family counters distinct. |

An exclusive pool is suitable for a query's candidate vector even when no query
answer can be reused. A cache is appropriate for prepared immutable geometry
only when the provider can define its key and validity. Neither facility replaces
the domain's traversal, decoding or rendering algorithm.

## Calling and ownership rules

The cache, publication coordinator and production map are caller serialized.
An owner phase is often convenient for Stock-style result application. A host
lock can also work, but key hashing/equality and direct payload destruction may
execute on that calling path. Do not hold a host lock across reentrant provider
submission or callbacks. Establish the association/ticket first, then invoke
the provider under an explicit host synchronization protocol.

SC production acceptance, SW admission, CPU completion, eligible cache
publication, provider consumption and final physical release are different
events. Keep each owner until the event that ends its own access. A notification
requests observation; it transfers neither ownership nor readiness.

Share backing for independent consumers. Borrow a view only within its owner's
valid borrow. Take when ownership should leave the table; taking can leave other
shared owners alive. Release unwanted candidates and displaced values through
their own cleanup contract. Pressure and cancellation cannot revoke an escaped
reader or an external device access.

## Examples

From the repository root, `cargo run --example NAME` runs a standalone example.

| Name | Demonstrates |
| --- | --- |
| [foundation](../examples/foundation.rs) | Shared backing, charges and owner-thread cleanup. |
| [keyed_cache](../examples/keyed_cache.rs) | Lookup, pins, eviction and identity survival. |
| [shared_production](../examples/shared_production.rs) | Provider decisions, input consumption and separate admissions. |
| [versioned_publication](../examples/versioned_publication.rs) | Captured dependencies, publication and retained old readers. |
| [reuse](../examples/reuse.rs) | Pools, copied ranges and consumed source buffers. |
| [manual_execution](../examples/manual_execution.rs) | Direct composition with a small executor and owned rejection. |
| [external_lifetime](../examples/external_lifetime.rs) | Input ownership beyond CPU completion through explicit final access. |
| [Separate SW package](../examples/solworker/README.md) | Real SW queue pressure, demand, discovery, owner publication and shutdown. |

The external-lifetime examples test ownership protocols with controlled readers;
they do not certify any particular GPU or audio backend's completion contract.
