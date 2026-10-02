# Research basis and limits

[Guide index](README.md)

The base case is the Stock 3.3.5a resource workload, informed by recovered Forever
mechanisms. The research used selected native paths and bounded offline checks.
The summaries here distinguish observations from the Rust contracts built from
them. They are not a claim of complete client recovery, identical algorithms,
native ABI compatibility or runtime performance measurements.

Some Forever bodies were unavailable. Where Stock supplies a concrete ownership
or service solution, the design uses that evidence while leaving the missing
Forever behavior unknown. A paired workload means a useful local correspondence;
it does not establish identical concurrency, keys or policy. All implementation
checks linked below run against public Rust APIs, not against a native client.

## Resource family correspondences

The [workload map](workloads.md#full-workload-map) applies these relationships to
concrete facilities. Where the counterpart is partial, the guide specifies a
provider boundary instead of inventing equivalent native behavior.

| Families | Recovered relationship | Mapping limit |
| --- | --- | --- |
| Models and sequences (M01–M02) | Both clients share model inputs separately from instances; released model revival/collection is distinct from active use. Stock shares external sequence production with independent playback users. | Forever's separate skeleton packaging does not require the same Stock resource graph. Complete skeleton cleanup was not recovered. |
| Textures and blobs (M03–M05) | Source identity and representation compatibility are distinct; blob/source backing, copied ranges and uploaded representations have different owners. Stock mount precedence and upload callbacks matter. | Exact range-store and temporary source-table counterparts in Stock were not proved. Complete outer unload exclusion remains a domain obligation. |
| World groups and spatial use (M08, M10–M13) | Shared roots/groups serve placements, queries and staged readiness; spatial consumers refresh shared demand independently of drawing. | Domain supplies residency windows, stable inputs and completeness. Not every Forever cutter/BSP mechanism has an exact Stock counterpart. |
| Animation and occlusion (M14–M15) | Per-instance preparation, useful joins and compatible retained capacity serve recurring frame work. | Matching storage or model identity does not establish matching pose/view history. Occlusion algorithms and execution arrangements are not asserted identical. |
| Graphics and materials (M16–M17) | Both have preparation, backend consumption and compatible storage reuse. Stock also has descriptor/declaration equality and redundant binding suppression. | Modern pipeline/material caching is partly an extension. CPU preparation/descriptor cleanup does not prove GPU retirement or all-reader exclusion. |
| Fonts (M18) | Glyph lookup precedes rasterization; atlas storage, placement-dependent text and temporary upload input have distinct lifetimes. Replacement can invalidate dependents. | Domain defines glyph/style identity and placement validity. Selected synchronous input reads do not prove every staging path has finished. |
| External retainers and execution records (M21–M22) | Resource consumers can outlive scene/table membership; worker records, notifications and delivery stages have their own lifetimes. | No mandatory Stock UI-retainer cache or SC job pool follows from the mapping. |

Composition (M06–M07), retention (M09), audio (M19) and metadata (M20) have dedicated
capsules below because their failure, acceptance or release paths directly shape
the public contracts.

## Composition

**Recovered:** Stock character composition separates changed-input detachment,
input readiness, compatible reusable output holders, worker production, owner
application and recycling. Forever separates active production allowance from
retained payload collection and has richer representation controls. Retained
inputs survive loss of current consumer interest.

**Adaptation:** use independent interest and production ownership, provider-defined
recipe/representation identity, exclusive output capacity, and an original
publication ticket checked on the owner. A new appearance request supersedes
publication eligibility without freeing the old worker's inputs.

**Limit:** shared output holders do not prove a content-keyed final-composite cache
across characters. Missing Forever publication internals are not filled in by
guessing. In the reviewed Stock path, completed-list insertion precedes a final
worker flag write; owner event service alone does not establish final access.
SC explicitly gates claim on all producer/access guards ending.

Implementation: [shared publication tests](../tests/publication_integration.rs),
[production contracts](README.md#production-and-consumer-demand).

## Sources

**Recovered:** Stock retains a selected archive owner under lookup protection and
rechecks final release under protection. File, I/O and archive layers have
separate owners. A successful outer close is not a universal transitive cleanup
receipt. Forever range copies, source buffers and catalog backing also have
different lifetimes.

**Adaptation:** select and own a source context coherently, retain it for dependent
reads/views, and bind cached bytes to explicit source identity/revision. New
lookups select the current context; old admitted users keep the old one.

**Limit:** retained storage does not prove unchanged content. The complete missing
Forever lookup-to-retain bridge and every source-reset caller remain unknown.

Implementation: [range coherence tests](../tests/ranges.rs),
[source buffers](../tests/source_buffers.rs).

## Loading and discovery

**Recovered:** Stock loading can discover terrain children, world-model groups
and model/texture inputs during later provider completion or owner service.
A drained CPU wave or provider queue can precede another discovery wave.
Forever prerequisite batches provide useful CPU ordering, with separate provider
and publication stages.

**Adaptation:** SW work sets and retained discovery permits cover dynamic execution
obligations; SC access guards cover actual input lifetime. Keep the host/provider
service alive until both obligations settle.

**Limit:** the observations do not require every asset to traverse one fixed
dependency chain. A CPU group join does not certify the whole load graph.

Implementation: [direct SW package](../examples/solworker/README.md).

## Demand

**Recovered:** world consumers refresh spatial demand; shared resources aggregate
interest. Forever stages request-priority changes and hands them to provider
queues separately. Stock file ordering favors lower numeric priorities on the
reviewed path. These provider queues are distinct from CPU worker classes.

**Adaptation:** domain urgency becomes SC independent interests, then an explicit
current-snapshot mapping to SW resource ranks or provider priority. Detaching a
placement removes its interest without necessarily canceling shared production.

**Limit:** native distance values and priority numbers are not SW API values.
Selected native staging has bounded/overflow behavior, not a universal lossless
update guarantee. The host must serialize forwarding or reject obsolete updates.

Implementation: [demand tests](../tests/demand.rs),
[actual SW execution-order checks](../examples/solworker/tests/composition.rs).

## Admission and provider completion

**Recovered:** Forever's reviewed provider starts with eight active slots and
accepts excess work into queues. Completion can invoke callbacks and synchronous
successors; slot release is separate from dependent processing and later resource
readiness. Stock distinguishes queued, busy, byte-ready and delivered requests.

**Adaptation:** separate production allowance, provider slots, SW queue capacity
and retained bytes. Preserve an uninvoked rejected operation for explicit retry
or settlement. Install ownership before calling a provider that may finish inline.

**Limit:** eight is an initial family setting, not a universal worker count or
queue cap. Complete Forever executor/release bridges, transitive close and all
exceptional reentrant paths were not recovered.

Implementation: [admission tests](../tests/admission.rs),
[owned rejection checks](../tests/direct_composition.rs).

## Queries and reusable capacity

**Recovered:** Forever query batches join before final result use while the caller
can perform independent gathering. Normal scratch return clears counts while
retaining capacity; free-list drain concerns available objects, not checked-out
users. Stock provides corresponding query workloads and mutable scratch, without
proving an identical task-group arrangement.

**Adaptation:** domain code supplies stable owned inputs or a valid exclusion
phase, SW partitions/joins work, and SC can retain prepared geometry or pool
compatible exclusive buffers. Missing residency is not a complete negative query.

**Limit:** native ordinary shared-memory accesses do not establish a safe Rust
synchronization protocol. Full scene mutation exclusion and all escaped native
view lifetimes were not proved. Reusing capacity does not validate old answers.

Implementation: [pool tests](../tests/pools.rs); domain-level scene algorithms
remain an application boundary, not an implemented generic SC query engine.

## Audio and consumed input

**Recovered:** the reviewed Stock audio open callback transfers an input handle
and clears its former slot. Creation rejection closes any input still in that
slot; the selected cache charge starts only on accepted creation, before later
readiness. Read activity and release synchronization have their own boundary.
Forever distinguishes removal of a family cache charge from later physical free.

**Adaptation:** rejection settles unconsumed inputs immediately. Already consumed
inputs remain with their accepted owner through actual cleanup. Serialize new
read admission against close, and retain charges through physical ownership.

**Limit:** this supplies a Stock-supported solution to missing Forever rejection
details, not recovery of those details. Neither every backend failure nor a
universal native no-new-read gate was established; SC closure is an explicit
stronger contract.

Implementation: [failure settlement](../tests/failure_settlement.rs),
[external lifetime](../tests/external_lifetime.rs), [closure](../tests/lifecycle.rs).

## Metadata authority

**Recovered:** selected positive-response handlers in both clients can create an
absent key. They are not limited to completing an existing pending request.
Bulk reset alone does not prove late-response origin checks or callback quiescence.

**Adaptation:** distinguish attempt completion from `SCPublication::authoritative`.
The provider establishes response eligibility through real transport context,
ordering or settlement. A local authority token checks only that local context.

**Limit:** a new local generation cannot reconstruct missing wire correlation.
Untagged delivery requires the caller to vouch for its current authority.

Implementation: [publication tests](../tests/publication.rs),
[authority contract](README.md#publication-and-declared-dependencies).

## Retention and external release

**Recovered:** model age retention, component payload collection, audio retirement
and graphics cleanup use different eligibility rules. Stock source cancellation
can detach an owner while a busy request retains cleanup responsibility. Logical
removal and callbacks do not universally establish last physical access.

**Adaptation:** independent owners survive table removal; collection targets only
eligible idle payloads. Provider/device retirement receipts control final release.
Close roots, settle descendants and deliveries with progress alive, then reclaim
what is actually unowned. Keep failures to stop externally owned work owned.

**Limit:** no global eviction policy, universal TTL or native whole-client drain
proof was recovered. The [family policies](policies.md) retain their distinct units.

Implementation: [retention](../tests/retention.rs),
[provider shutdown](../tests/shutdown.rs),
[SW shutdown composition](../examples/solworker/tests/shutdown.rs).
