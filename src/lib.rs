//! Resource caching and publication services.
//!
//! Initial module scaffold; resource behavior and public APIs are not implemented.
//! Intended responsibilities are described in `pubdocs/README.md`.

#![deny(unsafe_op_in_unsafe_fn)]

mod accounting;
mod cache;
mod demand;
mod dependency;
mod lifecycle;
mod ownership;
mod policy;
mod pool;
mod production;
mod progress;
mod publication;
mod range;
mod storage;
