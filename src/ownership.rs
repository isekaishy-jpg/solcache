//! Planned retained backing, scoped views and ownership transfer.
//!
//! Backing must survive every admitted access, including external use after
//! logical completion. Final release must respect its documented cleanup context.
