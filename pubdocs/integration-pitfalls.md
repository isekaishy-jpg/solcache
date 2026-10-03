# Integration failure points

[Guide index](README.md) · [Direct SW integration](solworker.md)

The base case is a Stock resource workload using Forever-informed policies.
SC and SW supply reusable mechanisms; the application's provider, resource
coordinator, host loop and renderer must establish the contracts between them.
An image match proves the sampled output, not every ownership or progress path.

## Measure the net frame-time effect

SC has nonzero lookup, ownership, accounting, demand and publication costs.
Calls made on the frame owner add synchronous work to that frame; work on other
threads can still delay a consumer or contend with it. Reuse and deduplication
should avoid more work than they introduce for the selected workload. A warm
comparison against an application that already caches and deduplicates the same
resources need not gain FPS merely by adding SC's stronger lifetime contracts.

Measure library lock waits separately from provider waits, SW queue residence,
owner service gaps and GPU retirement. An admission refusal is backpressure,
not time blocked inside that API. Instrumented wall spans can also contain
preemption and probe overhead; compare FPS and CPU using separate quiet runs.
An internal bookkeeping stall remains a library concern even when the host is
configured correctly.

Include statistics collection in that accounting. A host that resolves every
resident entry through the publication API each frame performs additional SC
work even when production and delivery are idle. Sample such diagnostics
deliberately and distinguish them from resource-service work. A long wall span
in a lookup is not sufficient to identify lock contention or CPU computation;
record thread execution/scheduling information when investigating isolated tails.

Bound publication and collection work in the host loop. A slot or record budget
limits how many items are visited, not the duration of an arbitrary payload
destructor. Direct backing destruction can run on the final releasing thread;
use an appropriate cleanup context and service phase when destruction is costly.
These host responsibilities include configuring the worker policy; see
[SW execution](https://github.com/isekaishy-jpg/solworker/blob/master/pubdocs/execution.md).

## Asset loading belongs to the provider

SC does not open files, select archives, issue OS asynchronous reads, decode
formats or upload textures. SW schedules CPU work and can represent external
readiness; `SWRuntime::external` does not implement a platform I/O backend.
The application supplies that backend, including immediate failure, inline
completion, cancellation and proof that physical access has ended.

For a synchronous archive reader, dedicated provider threads are one possible
composition. Keep blocking reads off caller-helpable CPU work. More provider
slots cannot make an archive handle concurrent if all reads take the same mutex.
The operation under that mutex may include lookup, decryption and decompression;
its elapsed time is not a measurement of physical disk latency alone.

Instrument the actual boundaries before attributing a stall:

| Interval | Investigate |
| --- | --- |
| Request to provider admission/start | Production allowance, provider slots, source priority and provider queue. |
| Before archive lock to lock acquired | Provider serialization and lock holders. |
| Lock acquired to bytes ready | The selected source implementation, including read/decrypt/decompress work. |
| Bytes ready to owner collection | Host service frequency and completion delivery. |
| Owner collection to decode admission/start | Retained retries, SW capacity, pending rank and worker availability. |
| Decode start to result | Format conversion and its allocations. |
| Result ready to publication/delivery | Owner phase service, publication budget, stale authority and consumer backlog. |
| Publication to upload submission/retirement | Upload budget, frame servicing, staging lifetime and actual device completion. |

Measure frame-start cadence and process CPU alongside these spans. A healthy CPU
worker pool cannot compensate for an owner loop that stops servicing completions.
Stock's separate queued, busy, byte-ready and delivered states support these
distinctions; see [provider evidence](evidence.md#admission-and-provider-completion)
and [loading waves](evidence.md#loading-and-discovery).

## Keep one authoritative demand path

Store each consumer's delivery address and SC interest handle together in the
coordinator. An application generation, last forwarded revision or cached
effective rank can be useful metadata; it should not become a second independent
interest policy beside SC.

Refresh the effective rank when consumers attach, change urgency or detach, then
forward it to the current provider/SW stage. Repeated identity resolution and
locked SC snapshots inside every queue-comparison call add avoidable work.
Serialize update and forwarding. Map SC's larger-is-urgent values explicitly to
SW's smaller-is-urgent ranks and drive bounded `service_demand`. A cached rank
without an invalidation/update rule is also incorrect.

Consumer interest does not pin a payload or cancel physical work by itself.
If the workload keeps CPU resources resident while a consumer is attached,
retain backing owners explicitly and release them with that residency contract.
See [demand evidence](evidence.md#demand).

## Carry ownership through actual work

Carry real inputs and the SC producer/access chain through provider and CPU
stages. The original publication ticket follows that production, including
rejected SW submissions and later retries. Retaining an owner-side unit token
does not by itself prove that worker/provider input access remains protected.

On SW `Full`, retain the returned uninvoked operation with its captures and
original authority. Reserve a successor where appropriate, or keep recoverable
pending state; never assume a read completion guarantees decode admission.
After source replacement, let old access settle and reject its old publication
authority. Detaching one consumer is a separate operation.

Account for the real allocations in the selected scope. Transfer temporary
output accounting to resident storage at installation; keep a backing owner
through upload consumption and any required device use. A copied pixel `Arc`
without its accounting owner can leave live bytes unreported. Release the owner
at the renderer's actual completion boundary, not merely on submission.

## Apply family policies with their actual units

The recovered 16-production and 32 MiB retention settings belong to component
production/payloads. Eight slots describe the reviewed provider family. They are
not universal limits for every BLP texture cache. Select policies for the mapped
family, and label any adaptation to a different workload explicitly.

Key-table capacity, active production count, provider operations, SW scheduling
capacity and retained bytes are independent controls. A cache-key bound should
not silently choose the production allowance. Service retention with a bounded
visit budget and report pressure that remains because of pins or the budget.
Usage above the retention threshold can be legitimate while readers hold owners;
it is not automatically a leak. See [policy contracts](policies.md).

## Keep progress alive through shutdown

Close new admissions, then continue provider, demand, owner and device service
until accepted operations and discovery obligations settle. A blocking provider
thread join on the owner can prevent the very callbacks needed to finish.
Requesting cancellation or observing an empty CPU queue does not establish
provider or device release. Keep rejected/unadmitted work distinct from accepted
work being drained; do not manufacture successful completion for either.

## Qualify comparisons and warm paths

A preloaded scene that bypasses SC is a useful control but cannot measure SC warm
reuse. Prime through the actual resource route, request the retained resources
again, and verify that those hits cause no new reads or decode productions.

Compare equivalent assets, scripted input, rendering and host instrumentation.
Report a matched-settings comparison separately from a policy change. Test
correctness, pressure, replacement and shutdown before interpreting timings.
Keep diagnostic runs separate: per-frame warning serialization, flushes and
stderr writes can change cadence, process CPU and completion-service intervals
even when they occur outside the reported frame-work span. A quiet run still has
the cost of any retained sampling/output and needs the same control instrumentation.

Report what ran: a synchronous MPQ provider on dedicated threads does not validate
a platform async backend; selected textures and frame workloads do not establish
coverage of all [mapped families](workloads.md).
