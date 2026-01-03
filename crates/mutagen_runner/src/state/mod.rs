//! State Module - Datenstrukturen für das Operator Pattern
//!
//! Dieses Modul enthält die Datenstrukturen für:
//! - **DesiredState**: Was laut Config existieren sollte
//! - **ActualState**: Was Mutagen tatsächlich sagt
//! - **ControllerState**: Minimaler Zustand des Controllers

mod actual;
mod controller;
mod desired;

pub use actual::{ActualSession, ActualState, SessionStatus};
pub use controller::{ControllerState, StagePhase};
pub use desired::{DesiredSession, DesiredState};
