//! Planned declared dependencies, revision tracking and invalidation.
//!
//! Callers identify dependencies and meaningful changes. Invalidation revokes
//! future reuse or publication while existing users retain their backing.
