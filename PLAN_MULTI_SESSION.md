# Plan: Multi-Session Bridge mit Keep-Alive

## Ziel
Die Bridge soll mehrere Prozesse gleichzeitig und nacheinander verwalten können, persistent laufen, und einen Keep-Alive Mechanismus für graceful shutdown haben.

## Protokoll-Änderungen

### Request (erweitert)
```rust
pub enum Request {
    Execute {
        session_id: u32,           // NEU
        program: String,
        args: Vec<String>,
        working_dir: Option<String>,
        env: Vec<(String, String)>,
        pty: Option<PtyConfig>,
    },
    Stdin {
        session_id: u32,           // NEU
        data: Vec<u8>,
    },
    Resize {
        session_id: u32,           // NEU
        cols: u16,
        rows: u16,
    },
    Signal {
        session_id: u32,           // NEU
        signal: i32,
    },
    Kill { session_id: u32 },      // NEU - Beendet spezifischen Prozess
    Ping,                          // NEU - Keep-alive
    Shutdown,
}
```

### Response (erweitert)
```rust
pub enum Response {
    Started { session_id: u32 },   // NEU - Bestätigung dass Prozess gestartet
    Output {
        session_id: u32,           // NEU
        stream: OutputStream,
        data: Vec<u8>,
    },
    Exit {
        session_id: u32,           // NEU
        code: Option<i32>,
    },
    Error {
        session_id: Option<u32>,   // NEU - None für globale Fehler
        message: String,
    },
    Pong,                          // NEU - Keep-alive response
    Ready,
}
```

## Bridge-Architektur

```
┌─────────────────────────────────────────────────────────┐
│                      Bridge                              │
│                                                          │
│  ┌──────────────┐    ┌─────────────────────────────┐   │
│  │ Main Loop    │    │ ProcessHandle HashMap       │   │
│  │              │    │                             │   │
│  │ - Read stdin │───→│ session_id → ProcessHandle  │   │
│  │ - Dispatch   │    │ session_id → ProcessHandle  │   │
│  │ - Keep-alive │    │ session_id → ProcessHandle  │   │
│  └──────────────┘    └─────────────────────────────┘   │
│         │                        │                      │
│         │            ┌───────────┴───────────┐         │
│         │            ▼                       ▼         │
│         │     ┌──────────┐            ┌──────────┐    │
│         │     │Process 1 │            │Process 2 │    │
│         │     │ stdout   │───┐        │ stdout   │──┐ │
│         │     │ stderr   │───┤        │ stderr   │──┤ │
│         │     └──────────┘   │        └──────────┘  │ │
│         │                    │                      │ │
│         │            ┌───────┴──────────────────────┘ │
│         ▼            ▼                                 │
│  ┌─────────────────────────┐                          │
│  │ Output Channel (mpsc)   │                          │
│  │ - Sammelt alle Outputs  │                          │
│  │ - Sendet zu stdout      │                          │
│  └─────────────────────────┘                          │
└─────────────────────────────────────────────────────────┘
```

## Keep-Alive Mechanismus

- **Ping-Intervall:** Client sendet alle 200ms
- **Timeout:** Nach 3 verpassten Pongs (~600ms) gilt Verbindung als tot
- **Bridge-Seite:** Beantwortet Ping sofort mit Pong
- **Bei Verbindungsabbruch:** Bridge beendet alle Prozesse graceful

## Implementierungs-Phasen

### Phase 1: Protokoll erweitern
- [ ] session_id zu allen relevanten Requests/Responses hinzufügen
- [ ] Kill { session_id } Request hinzufügen
- [ ] Ping/Pong hinzufügen
- [ ] Started Response hinzufügen
- [ ] PROTOCOL_VERSION auf 3 erhöhen

### Phase 2: Bridge Multi-Session
- [ ] HashMap<u32, ProcessHandle> für Prozess-Management
- [ ] Gemeinsamer Output-Channel für alle Prozesse
- [ ] Execute fügt Prozess zur HashMap hinzu
- [ ] Exit entfernt Prozess aus HashMap
- [ ] Kill beendet spezifischen Prozess
- [ ] Shutdown beendet alle Prozesse

### Phase 3: Keep-Alive in Bridge
- [ ] Ping → Pong Handler
- [ ] Idle-Timeout (optional, für den Fall dass Client stirbt ohne EOF)

### Phase 4: Client anpassen
- [ ] Session-ID Generator
- [ ] remote_run für session_id anpassen
- [ ] Keep-Alive Task im Hintergrund

### Phase 5: Testing
- [ ] Mehrere Prozesse gleichzeitig starten
- [ ] Prozesse einzeln killen
- [ ] Keep-Alive Verhalten testen
- [ ] Graceful shutdown bei Client-Abbruch

## Rückwärtskompatibilität

Breaking Change - PROTOCOL_VERSION wird erhöht. Da das Projekt noch jung ist, ist das akzeptabel.
