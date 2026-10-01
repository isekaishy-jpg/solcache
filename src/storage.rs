//! Planned internal record storage, identity and recycling.
//!
//! Record reuse must follow all final accesses and cannot grant old deliveries
//! authority over a new record. Storage layout and allocation strategy remain open.
