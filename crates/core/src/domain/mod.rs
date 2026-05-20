//! Domain types: pure data, no persistence, no FSM logic.
//!
//! Populated by T-0004. FSM behaviour lives in `state` (T-0005),
//! persistence in `inv-store` (T-0008), commands in `commands` (T-0011).

pub mod address;
pub mod creditnote;
pub mod customer;
pub mod ids;
pub mod invoice;
pub mod jurisdiction;
pub mod money;
pub mod reminder;
pub mod schedule;
