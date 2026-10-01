//! Planned independent consumer interest and current-production demand.
//!
//! Rejected attempts do not erase demand. Callers propagate urgency through their
//! execution services; this module does not schedule work or own worker threads.
