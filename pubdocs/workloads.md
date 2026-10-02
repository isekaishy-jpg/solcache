# Stock workload recipes

[Guide index](README.md)

These recipes apply Forever-informed mechanisms to Stock workloads. They define
where the generic APIs should be used; they do not assert that all client paths
have been ported or benchmarked. The [research basis](evidence.md) separates
recovered relationships from adaptation and remaining native limits.

## Full workload map

Every row identifies an SC facility and the execution/domain boundary. A product
needs a cache only when the domain can define reusable identity and validity.

| Family | SC use | SW and domain use |
| --- | --- | --- |
| M01 Shared models | `SCPublication`/`SCCache` for shared immutable products; backing pins and eligible idle retention. | SW prepares owned input; model provider parses. Placements and per-instance state stay separate. |
| M02 Sequences and skeletons | Shared production and retained animation dependencies. | Animation selects required sequences and packaging; independent playback clocks remain local. No artificial standalone Stock skeleton resource is required. |
| M03 Textures and representations | Separate source/CPU/device identity and dependencies; replacement preserves old readers. | SW performs legal CPU decode/preparation; renderer creates device objects and proves final use. |
| M04 Source availability and bytes | `SCSourceBuffers` for whole-buffer take; `SCRangeStore` for copied prefixes; owned source snapshots. | Provider selects source, performs I/O and defines freshness; SW resumes CPU work after readiness. |
| M05 Shared blobs | Shared backing and bounded views retain actual storage across consumers. | Provider owns layout and mount precedence; renderer's upload contract determines when input may be released. |
| M06 Character composition | Original attempt ticket, retained recipe inputs and `SCPool` output capacity. | SW executes composition; owner applies only the current result. Domain defines the recipe and whether content sharing is valid. |
| M07 Component variants and admission | Representation identity, `SCAdmission`, independent demand and overlapping candidate charges. | Provider selects required quality/channels. Do not create absent Stock HD work or equate native quality scales. |
| M08 WMO roots and groups | Shared roots/groups and stage-specific retained payloads. | SW prepares admitted stages; provider discovers groups/children. Placement, collision and rendering can need different readiness stages. |
| M09 Released retention | Share for revival; bounded collection of unpinned idle values; retiring charges. | Domain selects family policy; provider/device final use still gates destruction. |
| M10 Movement residency | Keep interests and owners for all selected cells/bands and nonvisual consumers. | World code computes residency windows; SW executes necessary preparation. Offscreen does not imply unused. |
| M11 Spatial urgency | Independent `SCDemand` interests aggregate for the current shared production. | Host explicitly maps current urgency to SW/provider order; world code supplies distance/importance rules. |
| M12 Culling outputs | Retain stable inputs; pool compatible result/scratch capacity; cache a prepared product only with valid declarations. | SW partitions and joins safe units. Scene owns visibility, visitation mutation and current-view acceptance. |
| M13 Collision, BSP and cutters | Prepared geometry, owned result backing, scratch pools and declared invalidation. | Domain owns traversal, masks, transforms, overlap and completeness. Missing resident geometry is not a complete negative answer. |
| M14 Animation and poses | Share immutable animation inputs; pool pose buffers; cache prepared results only with instance-valid keys. | SW runs independent evaluation; animation owns clocks, blending, events and dirty/needed-bone rules. |
| M15 Occlusion and far-world capacity | Retain persistent input and compatible CPU capacity. | Domain/renderer defines temporal validity and completion. Equal dimensions do not validate a previous visibility answer. |
| M16 Graphics and frame storage | Generic owners and charges for opaque values, with explicit deferred retirement. | Renderer owns heaps, command/descriptor pools, frames in flight and fences; SW CPU completion is insufficient. |
| M17 Pipeline, vertex formats and materials | Descriptor-key equality, failed/ready outcomes, publication and retention. | Renderer defines device namespace and compatibility. Binding suppression need not become a cache entry. |
| M18 Fonts and layout | Retain faces/glyph bytes, atlas/layout dependencies and reusable scratch. | Domain shapes, rasterizes and packs; placement changes invalidate dependent products. SW runs only safe CPU phases. |
| M19 Audio | Shared source/decoded products, consumed-input ownership, independent interests and retirement. | Provider owns decode/playback/stream state and callbacks. Never free input merely because a CPU task completed. |
| M20 Metadata and session | Scoped typed keys, failures and authoritative updates distinct from attempt completion. | Provider owns protocol authority, correlation and persistence. SW delivery does not certify response freshness. |
| M21 External retainers | Share backing beyond table removal and retain separate interest as needed. | UI/application defines retention policy; a dedicated UI cache is unnecessary. |
| M22 Worker records and notifications | Associate resources with owned attempts and observe progress. | SW owns job records, discovery and delivery. Host services notifications and checks actual state. |

## Model, texture and world loading

1. The provider chooses a source context and a semantic representation key.
   Under the coordinator's serialization, share a valid ready result first.
2. On a miss, join compatible pending production or start one with retained source
   inputs and a captured dependency snapshot. The
   [shared-production protocol](solworker.md#one-shared-production-one-publication-round)
   keeps one ticket with the outcome.
3. Admit I/O through the provider. When bytes are ready, run CPU preparation in
   SW without making workers block on the read. Keep admission/retry state owned
   if a required successor cannot enter SW yet.
4. Preparation or owner application may discover another source or child group.
   Retain SW discovery authority and SC input access for their distinct lifetimes.
5. Publish eligible CPU output. Rendering can then create a separate device
   product; retain upload input until the renderer's actual consumption boundary.
   Collision can use a CPU-ready product without waiting for renderability.
6. A source/device change invalidates the relevant future reuse. Old consumers
   retain their existing backing until their own final use.

Stock loading supplies the staged/discovered workload; Forever's mechanisms guide
the ownership and execution separation. This is not a requirement to keep source
bytes forever after safe derived production, nor to put all layers in one cache.
See [loading evidence](evidence.md#loading-and-discovery) and
[source evidence](evidence.md#sources).

## Appearance composition and retargeting

Use an owner-local attempt when the product is specific to one appearance state.
Use shared production only if a complete recipe and representation key actually
establishes reusable content. Stock's compatible output holders establish capacity
reuse, not cross-character final-image identity.

Retain required texture/model inputs and acquire compatible exclusive output
capacity. Apply the component production allowance separately from retained-byte
policy and SW admission. On appearance change, revoke the old publication round
and explicitly settle or supersede its production association. A joined request
must not create a replacement ticket for unchanged work.

Old work can finish safely and return its owned rejected candidate. Recycle its
inputs/output only after all access has ended; owner service and a completed flag
are not sufficient alone. The owner applies the accepted result, while renderer
upload and device retirement remain subsequent boundaries. See
[composition evidence](evidence.md#composition) and [family limits](policies.md).

## Frame queries, culling and animation

Capture stable scene/instance inputs or enter a domain phase that excludes
conflicting mutation. Check out compatible scratch/result capacity. Submit only
work whose writes and dependencies are understood; submitting Stock's mutable
traversal to SW does not make it thread safe.

Use the relevant SW group to join the results needed next. Independent caller
work can overlap that batch. Owner-only merge/application follows the join.
Do not reuse or return scratch while a result view, nested helper or external
consumer still accesses it. Reset counts/state explicitly before returning a
pool lease; nested backing has its own ownership and charges.

Cache prepared geometry or animation data only with domain-defined validity.
A pooled pose vector is capacity, not a reusable pose for every instance of the
same model. A retained visibility buffer is not proof of current visibility.
Queries must report incomplete residency when they cannot establish a complete
answer. See [query evidence](evidence.md#queries-and-reusable-capacity).

## Source rebuilds and partial reads

Use a whole-buffer take when a rebuild consumes retained source bytes. Use a
range copy when the caller owns a destination and can reuse a cached prefix.
Continue the remaining read against the same captured source context; never
combine a prefix from an old source with a suffix from its replacement.

Store valid bytes separately from retained allocation capacity. Preserve the
source-buffer family's read-size policy even when duplicate insertion returns
an incoming allocation. A source hit establishes bytes only; decode, owner
application and device work still have their own readiness and lifetime.
See [reuse example](../examples/reuse.rs) and [policy details](policies.md).

## Audio, metadata and other retained products

For audio, install input ownership before invoking creation. Rejection releases
unconsumed input; input already transferred stays with the consumer through
cleanup. Keep shared decoded data separate from each playback/stream cursor.
Closing reads requires an admission boundary and final-use settlement, including
callbacks and prefetch. See [audio evidence](evidence.md#audio-and-consumed-input).

For metadata, choose attempt publication for requested computation and
authoritative publication for provider-authorized updates, including creation of
absent membership. Associate transport context before delivery; local tokens
cannot identify old untagged messages. Distinguish absent, pending, failed and
ready. Failure retention, fallback and retry timing are domain policy, not an
automatic negative-cache TTL. See [metadata evidence](evidence.md#metadata-authority).

For fonts, materials and device products, the same ownership pattern applies
with different validity declarations: atlas placement, descriptor compatibility,
device generation or provider configuration. An external UI holder may keep the
old product alive through replacement. Give the renderer/provider its own final
release obligation instead of treating owner callback return as physical cleanup.

## What is validated

The [integration checks](solworker.md#evidence-and-executable-checks) validate the
generic handoffs with real SC/SW operations and controlled byte owners. This page
provides a concrete composition or provider boundary for every mapped family.
Full application workload scenarios, optimized benchmarks and flamegraphs are
still separate validation work. In particular, no native decode, traversal,
audio backend, GPU fence or network protocol is implemented by these recipes.
