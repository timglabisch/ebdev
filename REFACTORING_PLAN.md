# Refactoring Plan: Operator Pattern für Mutagen Sync

## Aktueller Stand

Die aktuelle Architektur ist **prozedural**:
- `run_staged_sync_impl` ist eine große Funktion (~100 Zeilen) mit verschachtelter Logik
- State-Berechnung und Ausführung sind vermischt
- Reconciliation-Logik verteilt auf `sync_stage_sessions` und `cleanup_removed_projects`
- Testen erfordert Mocks für das MutagenBackend

## Ziel-Architektur: Operator Pattern

```
┌─────────────────────┐     ┌─────────────────────┐
│    DesiredState     │     │    ActualState      │
│  (aus Config/Plan)  │     │   (von Mutagen)     │
└──────────┬──────────┘     └──────────┬──────────┘
           │                           │
           └───────────┬───────────────┘
                       ▼
           ┌─────────────────────┐
           │     reconcile()     │  ← PURE FUNCTION
           │   (Ist vs Soll)     │
           └──────────┬──────────┘
                      │
                      ▼
           ┌─────────────────────┐
           │   Vec<Action>       │
           │ Create/Terminate/   │
           │ Wait/Advance/Done   │
           └──────────┬──────────┘
                      │
                      ▼
           ┌─────────────────────┐
           │   SyncController    │  ← Hält minimalen State
           │   (führt aus)       │
           └─────────────────────┘
```

## Datenstrukturen

### DesiredState (Soll-Zustand)

```rust
/// Was laut Config existieren sollte
#[derive(Debug, Clone)]
pub struct DesiredState {
    pub sessions: Vec<DesiredSession>,
    pub stages: Vec<i32>,           // Sortierte Stage-Nummern
    pub last_stage: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DesiredSession {
    pub name: String,               // Session-Name (inkl. CRC32)
    pub project_name: String,       // Projekt-Name ohne CRC32
    pub alpha: PathBuf,             // Lokaler Pfad
    pub beta: String,               // Remote Target
    pub mode: SyncMode,
    pub stage: i32,
    pub ignore: Vec<String>,
    pub polling: PollingConfig,
    pub config_hash: u64,           // Hash der relevanten Config-Felder
}
```

### ActualState (Ist-Zustand)

```rust
/// Was Mutagen tatsächlich sagt
#[derive(Debug, Clone, Default)]
pub struct ActualState {
    pub sessions: Vec<ActualSession>,
}

#[derive(Debug, Clone)]
pub struct ActualSession {
    pub identifier: String,         // Mutagen Session ID
    pub name: String,               // Session Name
    pub status: SessionStatus,
    pub alpha: String,
    pub beta: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionStatus {
    Connecting,
    Scanning,
    Syncing,
    WaitingForRescan,
    Watching,
    Error(String),
    Unknown(String),
}
```

### Actions

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Session erstellen
    Create {
        session: DesiredSession,
        no_watch: bool,
    },
    /// Session terminieren
    Terminate {
        identifier: String,
        name: String,
    },
    /// Warten bis Sessions complete sind
    WaitForCompletion {
        session_names: Vec<String>,
    },
    /// Stage wurde abgeschlossen, Sessions terminieren
    FinalizeStage {
        stage: i32,
        session_names: Vec<String>,
    },
    /// Zur nächsten Stage wechseln
    AdvanceStage {
        from: i32,
        to: i32,
    },
    /// In Watch-Mode gehen (final stage mit keep_open)
    EnterWatchMode,
    /// Fertig
    Complete,
}
```

### Controller State

```rust
#[derive(Debug, Clone)]
pub struct ControllerState {
    pub current_stage: Option<i32>,
    pub stage_phase: StagePhase,
    pub options: SyncOptions,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StagePhase {
    /// Noch nicht gestartet
    NotStarted,
    /// Sessions werden synchronisiert
    Syncing,
    /// Warten auf Completion
    WaitingForCompletion,
    /// Stage abgeschlossen, Sessions werden terminiert
    Finalizing,
    /// Im Watch-Mode (final stage)
    Watching,
    /// Fertig
    Done,
}
```

## Die reconcile() Funktion - Pure Function!

```rust
/// Berechnet die nächsten Aktionen basierend auf Ist/Soll-Zustand.
///
/// PURE FUNCTION - keine Side Effects, perfekt testbar!
pub fn reconcile(
    desired: &DesiredState,
    actual: &ActualState,
    state: &ControllerState,
) -> Vec<Action> {
    // ... Logik
}
```

### Reconciliation-Logik

1. **Phase: NotStarted**
   - Cleanup: Sessions für gelöschte Projekte terminieren
   - Wenn stages_to_run leer → Complete
   - Sonst → AdvanceStage zur ersten Stage

2. **Phase: Syncing**
   - Für aktuelle Stage: Sessions erstellen/behalten/ersetzen
   - Wenn alle Sessions erstellt → WaitForCompletion

3. **Phase: WaitingForCompletion**
   - Prüfe ob alle Sessions complete sind
   - Wenn ja und is_final && keep_open → EnterWatchMode
   - Wenn ja und is_final && !keep_open → Complete
   - Wenn ja und !is_final → FinalizeStage

4. **Phase: Finalizing**
   - Sessions der Stage terminieren
   - AdvanceStage zur nächsten Stage (oder Complete)

5. **Phase: Watching**
   - Keine Aktionen (User muss quiten)

## Dateistruktur nach Refactoring

```
crates/mutagen_runner/src/
├── lib.rs                 # Public API, re-exports
├── state/
│   ├── mod.rs
│   ├── desired.rs         # DesiredState, DesiredSession
│   ├── actual.rs          # ActualState, ActualSession, SessionStatus
│   └── controller.rs      # ControllerState, StagePhase
├── reconcile/
│   ├── mod.rs
│   ├── actions.rs         # Action enum
│   ├── reconcile.rs       # reconcile() pure function
│   └── tests.rs           # Unit tests für reconcile
├── controller/
│   ├── mod.rs
│   ├── sync_controller.rs # SyncController
│   └── executor.rs        # Action execution
├── backend/
│   ├── mod.rs
│   ├── trait.rs           # MutagenBackend trait
│   ├── real.rs            # RealMutagen
│   └── mock.rs            # MockMutagen (für Tests)
├── ui/
│   ├── mod.rs
│   ├── trait.rs           # SyncUI trait
│   ├── headless.rs        # HeadlessUI
│   └── tui.rs             # TuiUI
├── status.rs              # MutagenSession (bestehend)
└── tui.rs                 # TUI helpers (bestehend)
```

## Implementierungsplan

### Phase 1: Datenstrukturen (Tag 1)

**Schritt 1.1: State-Modul erstellen**
- [ ] `state/desired.rs` - DesiredState, DesiredSession
- [ ] `state/actual.rs` - ActualState, ActualSession, SessionStatus
- [ ] `state/controller.rs` - ControllerState, StagePhase
- [ ] `state/mod.rs` - Re-exports

**Schritt 1.2: Actions definieren**
- [ ] `reconcile/actions.rs` - Action enum mit allen Varianten
- [ ] Tests für Action equality/debug

### Phase 2: Reconcile-Funktion (Tag 2)

**Schritt 2.1: Core reconcile()**
- [ ] `reconcile/reconcile.rs` - Hauptlogik
- [ ] Cleanup-Logik (gelöschte Projekte)
- [ ] Stage-Sync-Logik (create/keep/replace)
- [ ] Completion-Detection
- [ ] Stage-Transition-Logik

**Schritt 2.2: Umfangreiche Tests**
- [ ] `reconcile/tests.rs` - Unit Tests
- [ ] Test: Neue Session erstellen
- [ ] Test: Bestehende Session behalten
- [ ] Test: Config-Änderung → Replace
- [ ] Test: Gelöschtes Projekt → Terminate
- [ ] Test: Multi-Stage Workflow
- [ ] Test: Init-Only Mode
- [ ] Test: Watch-Mode Entry

### Phase 3: Controller (Tag 3)

**Schritt 3.1: SyncController**
- [ ] `controller/sync_controller.rs`
- [ ] Hauptloop: fetch → reconcile → execute → repeat
- [ ] Error handling
- [ ] User abort detection

**Schritt 3.2: Action Executor**
- [ ] `controller/executor.rs`
- [ ] Execute Create
- [ ] Execute Terminate
- [ ] Execute WaitForCompletion (mit polling)
- [ ] Execute AdvanceStage
- [ ] Execute EnterWatchMode

### Phase 4: Integration (Tag 4)

**Schritt 4.1: Backend refactoring**
- [ ] `backend/trait.rs` - Vereinfachtes MutagenBackend
- [ ] `backend/real.rs` - RealMutagen (minimal changes)
- [ ] `backend/mock.rs` - MockMutagen für Tests

**Schritt 4.2: UI Integration**
- [ ] Controller → UI Events mapping
- [ ] Bestehende UI-Implementierungen anpassen

**Schritt 4.3: Public API**
- [ ] `lib.rs` aktualisieren
- [ ] Bestehende public functions migrieren
- [ ] Backwards compatibility (falls nötig)

### Phase 5: Tests & Cleanup (Tag 5)

**Schritt 5.1: Integration Tests**
- [ ] Full-Flow Tests mit MockMutagen
- [ ] Multi-Stage Szenarien
- [ ] Error Handling Szenarien

**Schritt 5.2: Cleanup**
- [ ] Alte Implementierung entfernen
- [ ] Unused code entfernen
- [ ] Documentation

## Beispiel: reconcile() Test

```rust
#[test]
fn test_reconcile_creates_new_session() {
    // Desired: Eine Session soll existieren
    let desired = DesiredState {
        sessions: vec![DesiredSession {
            name: "frontend-abc123".into(),
            project_name: "frontend".into(),
            alpha: PathBuf::from("/local/frontend"),
            beta: "docker://app/frontend".into(),
            mode: SyncMode::TwoWay,
            stage: 0,
            ignore: vec![],
            polling: PollingConfig::default(),
            config_hash: 12345,
        }],
        stages: vec![0],
        last_stage: 0,
    };

    // Actual: Keine Sessions existieren
    let actual = ActualState::default();

    // State: Stage 0, Syncing Phase
    let state = ControllerState {
        current_stage: Some(0),
        stage_phase: StagePhase::Syncing,
        options: SyncOptions::default(),
    };

    // Reconcile
    let actions = reconcile(&desired, &actual, &state);

    // Erwarte: Create Action
    assert_eq!(actions.len(), 1);
    assert!(matches!(actions[0], Action::Create { .. }));
}

#[test]
fn test_reconcile_keeps_matching_session() {
    let desired = /* ... Session "frontend-abc123" ... */;

    // Actual: Session existiert bereits mit gleichem Namen
    let actual = ActualState {
        sessions: vec![ActualSession {
            identifier: "sess-1".into(),
            name: "frontend-abc123".into(),
            status: SessionStatus::Watching,
            alpha: "/local/frontend".into(),
            beta: "docker://app/frontend".into(),
        }],
    };

    let state = /* Stage 0, Syncing */;

    let actions = reconcile(&desired, &actual, &state);

    // Erwarte: Keine Create/Terminate Actions
    assert!(actions.iter().all(|a| !matches!(a, Action::Create { .. })));
    assert!(actions.iter().all(|a| !matches!(a, Action::Terminate { .. })));
}
```

## Vorteile nach Refactoring

1. **Testbarkeit**: `reconcile()` ist pure → Unit Tests ohne Mocks
2. **Lesbarkeit**: Klare Trennung von Ist/Soll/Aktionen
3. **Debugbarkeit**: "Warum diese Aktion?" → Vergleiche Ist mit Soll
4. **Robustheit**: Idempotent, kann jederzeit neu reconcilen
5. **Erweiterbarkeit**: Neue Aktionen einfach hinzufügen
6. **Dry-Run**: Trivial - einfach Actions anzeigen statt ausführen

## Risiken & Mitigationen

| Risiko | Mitigation |
|--------|------------|
| Breaking Changes | Bestehende public API als Wrapper behalten |
| Regression | Bestehende Tests als Acceptance Tests behalten |
| Performance | Reconcile ist schnell (nur Datenstrukturen) |
| Komplexität | Kleine, fokussierte Module |

## Nächster Schritt

Starte mit **Phase 1: Datenstrukturen** - diese sind die Grundlage für alles weitere.
