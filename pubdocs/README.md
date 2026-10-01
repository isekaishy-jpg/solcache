# Solcache

Solcache is a Rust library project for reusable resource caching and publication
services. This repository currently contains scaffolding only; no cache API or
implementation is available yet, and dependencies have not been selected.

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
