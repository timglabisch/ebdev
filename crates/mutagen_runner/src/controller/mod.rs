//! Controller Module - Operator Pattern Controller
//!
//! Dieses Modul enthält den SyncController der den Operator Pattern Loop ausführt:
//! 1. Hole aktuellen Zustand (ActualState)
//! 2. Berechne Actions (reconcile)
//! 3. Führe Actions aus
//! 4. Wiederhole bis fertig

pub mod executor;
pub mod sync_controller;

pub use executor::{execute_action, ExecuteResult};
pub use sync_controller::SyncController;
