# Direct Solcache and Solworker composition

This standalone example package uses the sibling Solworker checkout directly.
Solcache's library and normal dependency graph remain independent of Solworker.
The three workers, queue windows and byte counts are instructional capacities,
not measured performance results or deployment recommendations.

From the Solcache repository root, with both checkouts as siblings:

```powershell
cargo run --locked --manifest-path examples/solworker/Cargo.toml
cargo test --locked --manifest-path examples/solworker/Cargo.toml
cargo clippy --locked --manifest-path examples/solworker/Cargo.toml --all-targets -- -D warnings
```

The package exercises real public SC and SW operations:

- A coordinator first checks valid cache reuse, calls `begin_shared` only on a
  miss, and creates a publication ticket only for `Started`. Joined input returns
  intact, and the original ticket stays paired with that production's output.
- A channel-gated worker plus queued work fills the runnable window. The root
  resource stage has ordinary admission; its independent output-byte reservation
  does not reserve a CPU job or bypass runnable pressure. Rejection
  retains the exact untouched operation, SC root and SW options. SC `Accepted`
  means the **host accepted responsibility for the retained retry**; it does not
  mean SW queue admission succeeded. One retry follows explicit queue progress.
  A separate case drops the retained closure, settles SC abandonment and releases
  both real input ownership and the production allowance. No retry loop runs.
- SC demand has larger urgency values for higher priority. The example maps
  urgency at least 10 to SW rank 0 and other urgency to rank 10. The coordinator
  serializes every demand update and forwarding operation, rejects obsolete
  snapshots and forwards a current snapshot. A competing pending stage proves
  the mapped demand changes actual execution order. Dropping interest does not
  cancel owned producer or child accesses.
- A live SW discovery permit admits an SC child input owner after root closure.
  Root CPU completion still leaves the output unclaimable until that child's
  actual access ends. This is a CPU continuation with owned bytes; no device or
  foreign provider completion is simulated as a physical retirement receipt.
- A genuine `SWOwner::on_ready` callback claims the original SC output and
  publishes in its selected phase. The worker output holds a detached SW byte
  lease acquired through `SWReservation::stage` and `retain_bytes`. The owner
  acquires an SC destination charge before `SWRetained::into_inner` releases the
  SW lease, then binds it through `SCCleanupContext::acquire_charged`. This explicit
  transfer has a brief declared-accounting overlap; the two accounting domains
  are not summed as one physical-memory total. Allocation addresses verify that
  ownership moves do not copy the input or output bytes.
- An additional real-byte pipeline case forces SC's representational overflow
  with a synthetic `u64::MAX` declared charge after input final use. This sentinel
  is arithmetic test data, not allocated memory. Rejection retains the exact
  SW output, its byte lease and original publication ticket. Removing the sentinel
  permits one explicit owner-local retry. Publication rejection likewise leaves
  its owned candidate and original ticket in owner state for caller handling.
- Scoped cleanup backing is created on the owner, since it cannot be captured
  by SW jobs requiring `'static` ownership. A retained reader survives cache
  clearing; its final release queues cleanup and the owner drains it. The actual
  payload destructor records its executing thread. Warm valid lookup performs
  no producer start, ticket creation, worker submission or delivery.

Tests use channels and completion groups to select interleavings. Timeouts only
bound a failed wait; no sleeps determine correctness. The runnable application
and integration tests share these example scenarios, with tests under `tests/`.

Registering an SW owner forbids SW passive waits on that thread, even outside a
callback. Before registration the example can wait on CPU groups; afterward it
uses bounded nonblocking status observation for the independent child group,
then pumps the owner until delivery completes. Terminal group status alone does
not certify subscriber activation. This finite fixture is not a general host event
loop. The root package's checks do not include this separate manifest. Add
`--offline` to these commands when Solworker's locked dependencies are cached.

The separate [shutdown case](tests/shutdown.rs) closes SC root admission and
starts SW shutdown while an accepted producer is gated. `try_shutdown` reports
incomplete settlement while the host retains and pumps owner delivery. The output
is then moved out of the owner, remains usable after worker join, and retires only
after its final SC owner is released and bounded cleanup is serviced. This test
does not introduce a shutdown coordinator or another execution abstraction.
