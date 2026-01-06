//! State Module - Datenstrukturen für das Operator Pattern
//!
//! Dieses Modul enthält die Datenstrukturen für:
//! - **DesiredState**: Was laut Config existieren sollte
//! - **ActualState**: Was Mutagen tatsächlich sagt

mod actual;
mod desired;

pub use actual::{ActualSession, ActualState, SessionStatus};
pub use desired::{DesiredSession, DesiredState};
