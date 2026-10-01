//! Planned exclusive reusable object and buffer checkouts.
//!
//! Compatibility, reset, retained capacity and final borrower access govern reuse.
//! Closing drains idle objects while later returns retain the state for cleanup.
