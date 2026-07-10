# tws Mobile — Implementierungsplan (iOS only)

Stand: 2026-07-04. Ziel: iOS-App (Expo), die sich mit einem oder mehreren Macs
verbindet, dort die tws-Hierarchie (Collection → Thread → Session → Agent) anzeigt,
verwaltet und interaktive Terminals auf tmux-Sessions öffnet.

## Ziele

- Multi-Host: App startet mit Host-Liste, Hosts hinzufügbar (QR-Pairing)
- Live-Dashboard: Tree-View mit Agent-Status + semantischem Pi-Status (Working/Retry/Erfolg/Abbruch/Unvollständig/Fehler)
- CRUD: Collections/Threads anlegen/umbenennen/löschen, Sessions erstellen/killen/umbenennen
- Notes lesen (Markdown)
- Interaktives Terminal auf jede tmux-Session, natives Rendering (libghostty via Termini)
- Read-only-Modus fürs "Zugucken" bei laufenden Agents

## Nicht-Ziele (v1)

- Android (später: ghostty-web WebView hinter gleichem Interface)
- Push-Notifications (v1.1-Kandidat, braucht Relay)
- Terminal-Splits/Windows-Management in der App (tmux-Windows werden 1:1 angezeigt, kein eigenes Layouting)
- Öffentliche Erreichbarkeit — Transport ist Tailnet/LAN

## Architektur

```
┌─ Mac ─────────────────────────────────────┐        ┌─ iPhone ───────────────────┐
│ Cargo-Workspace                           │        │ Expo-App (Dev Client)      │
│  ├─ tws-core   lib: model/state/tmux/…    │        │  ├─ Hosts / Dashboard (TS) │
│  ├─ tws        TUI-Binary wie bisher      │  REST  │  └─ TerminalView           │
│  └─ tws-server axum-Binary                │◄──────►│                            │
│      ├─ REST: state, CRUD, notes          │   WS   │     Expo-Module (Swift)    │
│      ├─ WS  /events: Status-Push          │◄──────►│     Termini + GhosttyKit   │
│      └─ WS  /attach: PTY ↔ Bytes          │◄──────►│     WS nativ (kein Bridge- │
│           portable-pty → tmux attach      │Tailnet │     Byte-Traffic)          │
└───────────────────────────────────────────┘        └────────────────────────────┘
```

### Zentrale Entscheidungen

| Entscheidung | Begründung |
|---|---|
| Cargo-Workspace: `tws-core` (lib) + `tws` (TUI) + `tws-server` (axum) | Saubere Trennung Domain/UI/Server; kein Upstream-Support nötig (Fork divergiert bewusst). tokio/axum bleiben aus dem TUI-Build raus |
| Server statt SSH | Kein brauchbarer SSH-Stack in RN; Logik bliebe sonst auf dem Handy; keine Live-Pushes; state.json-Races |
| WS lebt nativ (Swift), nicht in JS | High-Frequency-PTY-Bytes über die RN-Bridge = Perf-Killer. JS gibt nur Props, Swift öffnet eigenen WS |
| Termini (arach/Termini) für Terminal-Rendering | libghostty/Metal nativ, "bring your own transport" passt exakt auf unseren WS. Fallback: Lakr233/libghostty-spm direkt wrappen |
| Grouped Sessions beim Attach | Unabhängige Terminal-Größe pro Client; Mac-TUI schrumpft nicht, wenn das iPhone attached |
| ts-rs für Typen | Rust-Structs → generierte TS-Types, kein manueller Sync |

## WS-Protokolle (Vertrag, Phase 2/3 fixiert)

**`/api/events`** (Status-Push): Text-Frames, JSON.
Server pusht vollständigen State-Snapshot bei Änderung (Mutation via REST,
Agent-Scan-Tick, state.json-Änderung durch TUI). v1 = Snapshot, kein Diffing.

**`/api/sessions/:name/attach?readonly=0|1`** (Terminal):
- Binary-Frames = rohe PTY-Bytes, beide Richtungen
- Text-Frames = Control-JSON:
  - Client→Server: `{"type":"resize","cols":N,"rows":N}`, `{"type":"ping"}`
  - Server→Client: `{"type":"exit","code":N}`, `{"type":"pong"}`
- Read-only wird **serverseitig** erzwungen (Input-Bytes verwerfen + `attach -r`), nicht nur UI

**Auth**: Bearer-Token (Header, bei WS zusätzlich Query-Param-Fallback).
Token liegt in `~/.config/tws/server.toml`, wird bei erstem `tws-server`-Start generiert.

---

## Phase 0 — De-Risk-Spike: Termini + Custom Transport (~1 Tag)

Das riskanteste Stück zuerst validieren, bevor irgendwas anderes gebaut wird.

- [ ] Mini-SwiftUI-App (Xcode, wegwerfbar): `TerminiTerminalView` + `TerminiTerminalController` mit eigenem Transport
- [ ] Dummy-Server (z. B. 20 Zeilen Python/Bun: WS → lokales PTY mit `tmux attach`)
- [ ] Prüfen: Rendering-Qualität, Resize, Farben/TUIs (pi, vim), Latenz über Tailscale, Keyboard-Input inkl. Sonderzeichen
- [ ] Prüfen: Termini-Transport-API — reicht `write/onData/resize` oder fehlt was?

**Abbruchkriterium**: Wenn Termini nicht trägt → GhosttyKit direkt via libghostty-spm
wrappen (Termini-Quellcode als Referenz). Wenn auch das scheitert → ghostty-web in
WebView (Plan bleibt sonst identisch).

## Phase 1 — Refactor: Workspace-Split + Persistence-Härtung (~1–2 Tage)

Kein Verhaltens-Change, alle 75 Tests grün, TUI identisch.

Ziel-Layout:

```
Cargo.toml            # [workspace] members = ["crates/*"]
crates/
  tws-core/           # lib: core/{model,state,persistence,pi_status,notes},
                      #      tmux/{commands,agent_scan}, git/worktrees,
                      #      config-Parsing (config.toml → Struct)
  tws/                # bin "tws": app, components, theme, tui, event, import,
                      #      markdown (tui-markdown), keymap-/palette-Resolution
  tws-server/         # bin "tws-server": Phase 2/3 (zunächst leeres Skeleton)
```

- [ ] Workspace-Root-`Cargo.toml`, Crates verschieben (`git mv`, Historie erhalten)
- [ ] Schnitt: `tws-core` = alles ohne ratatui/crossterm-Dependency; `markdown.rs`, `theme.rs`, `config/{keys,palette}` bleiben im TUI-Crate (ratatui-gebunden); `config`-Datei-Parsing (start_dirs, worktrees) nach core, Keymap/Palette-Auflösung bleibt in `tws`
- [ ] Sichtbarkeiten: `pub(crate)` → `pub` wo nötig (z. B. `persistence::config_dir`)
- [ ] `persistence::save()` atomic machen: write nach `state.json.tmp` + `fs::rename` (gilt auch für `save_ui`)
- [ ] state.json-Reload im TUI: mtime-Poll im bestehenden 250ms-Tick (kein notify-Crate nötig); bei Fremdänderung neu laden, Selektion best-effort erhalten
- [ ] Regressiontests: atomic write, reload-on-change; bestehende Tests wandern mit ihren Modulen
- [ ] Release-Prozess in CLAUDE.md anpassen (Version-Bump betrifft jetzt Workspace-Crates)
- [ ] `cargo build && cargo test` grün; TUI-Smoke-Test manuell

## Phase 2 — `tws-server`: REST + Events + Pairing (~3–4 Tage)

Crate `crates/tws-server`: tokio, axum, tower-http, ts-rs (dev), Dependency auf `tws-core`.

- [ ] `src/main.rs` + `routes.rs`, `auth.rs`, `events.rs`
- [ ] CLI: `tws-server [--addr 127.0.0.1:8712]`, `tws-server --pair` (QR im Terminal: `tws://host:port#token`)
- [ ] Server-State: `Arc<RwLock<AppState>>`; Hintergrund-Tasks:
  - Agent-Scan-Loop (Wiederverwendung `scan_agents` + Pi-Status, wie TUI-30s-Tick)
  - Session-Refresh (`list_tws_sessions_with_timestamps` → `refresh_sessions`)
  - state.json-Watch (mtime) für TUI-Änderungen
- [ ] REST-Endpoints:
  - `GET  /api/state` — kompletter Tree-Snapshot (Collections inkl. Sessions/Agents/Pi-Indikatoren, Recent)
  - `POST /api/collections`, `PATCH/DELETE /api/collections/:id`
  - `POST /api/collections/:id/threads`, `PATCH/DELETE /api/threads/:id`
  - `POST /api/sessions` (Thread + Label + optional start_dir → `make_session_name` + `new_session_in_dir`)
  - `DELETE /api/sessions/:name` (kill), `PATCH /api/sessions/:name` (rename)
  - `GET  /api/threads/:id/note`, `PUT /api/threads/:id/note`
  - `GET  /api/health` (Version, Hostname — für Host-Liste in der App)
- [ ] `WS /api/events`: Snapshot-Push bei jeder Änderung, Broadcast an alle Clients
- [ ] Auth-Middleware (Bearer), Token-Generierung in `~/.config/tws/server.toml`
- [ ] ts-rs: `#[derive(TS)]` auf DTO-Structs (eigene DTOs, nicht die Domain-Structs — Fork-Diff klein halten), Export-Test generiert `.ts` nach `bindings/`
- [ ] Tests: Auth (401), CRUD-Roundtrip gegen temp-Config-Dir, Events-Broadcast
- [ ] launchd-Service deklarativ in `~/.config/nixos` (nix-darwin) — Doku im Plan, Umsetzung separat

## Phase 3 — Terminal-Attach: PTY ↔ WS (~2–3 Tage)

- [ ] Dependency `portable-pty` in `tws-server`
- [ ] `crates/tws-server/src/attach.rs`: pro WS-Verbindung
  1. Grouped Session: `tmux new-session -t <target-group> -s <target>-m<rand>` (unabhängige Größe/Window-Wahl), bei `readonly=1` → `attach -r`-Semantik
  2. PTY spawnen mit `attach-session -t <mirror>`, Größe aus initialem resize-Frame
  3. Pump-Tasks: PTY→WS (binary), WS→PTY (binary, bei readonly verwerfen), Control-Frames (resize/ping)
  4. Cleanup bei Disconnect: PTY killen, Mirror-Session `kill-session`
- [ ] Verwaiste Mirror-Sessions beim Serverstart aufräumen (Namenspräfix-Scan)
- [ ] Timeout/Heartbeat: Ping alle 30s, tote Verbindungen schließen
- [ ] Tests: Mirror-Lifecycle (create/cleanup), readonly-Enforcement, Resize-Handling (Protokoll-Ebene; PTY-Pump manuell via wscat/Spike-Client)

## Phase 4 — Expo-App (iOS) (~1–2 Wochen)

Neues Repo: `~/dev/Personal/mobile-apps/tws-mobile` (Setup analog hackerschau:
Expo + TypeScript + NativeWind + expo-router + flake.nix; Dev Client ab Tag 1,
kein Expo Go — natives Modul geplant).

### 4a — Foundation & Hosts

- [ ] Repo-Scaffold, `expo prebuild`, Dev-Client-Build lokal (expo-local-build-without-eas-Skill)
- [ ] Host-Modell: `{ id, name, url, token }` — Storage: expo-secure-store (Token!) + AsyncStorage (Metadaten)
- [ ] Screens: Host-Liste (Status-Dot: online/offline via `/api/health`), Host hinzufügen (manuell URL+Token / QR-Scan via expo-camera → `tws://`-URI parsen)
- [ ] API-Client aus ts-rs-Bindings, TanStack Query; WS-`/events`-Hook mit Reconnect/Backoff pro Host

### 4b — Dashboard

- [ ] Tree-Screen pro Host: Collections → Threads → Sessions → Agents, collapsible
- [ ] Status-Badges: Agent-Typ (claude/codex), semantische Pi-Statusmarker (Parität zur TUI)
- [ ] Recent-Sessions-Row (Analog `recent_bar`)
- [ ] Aktionen (Context-Menü/Swipe): Session erstellen (Thread + Label + start_dir), killen (Confirm), umbenennen; Collection/Thread CRUD
- [ ] Notes-Viewer (react-native-markdown o. ä., read-only in v1)
- [ ] Offline-/Error-States pro Host (letzter Snapshot + "stale"-Hinweis)

### 4c — Terminal (natives Modul)

- [ ] Local Expo Module `modules/tws-terminal` (expo-modules-core, Swift)
  - SwiftPM-Dependency: Termini (Version pinnen; GhosttyKit-xcframework kommt prebuilt)
  - View-Props: `{ wsUrl, token, readOnly, appearance }` — Swift öffnet `URLSessionWebSocketTask` selbst
  - Transport-Adapter: WS-Binary → `controller.write`, Input → WS-Binary, Größenänderung → resize-Frame
  - Events an JS: `onConnected`, `onDisconnected(reason)`, `onExit(code)`, `onBell`, `onTitleChange`
- [ ] Terminal-Screen: Modul-View + Key-Accessory-Row (`Esc` `Ctrl` `Tab` `↑↓←→` `-` `/` Paste), Read-only-Toggle, Disconnect-Banner mit Retry
- [ ] Lifecycle: Background → WS trennen, Foreground → Reattach (Mirror-Session ist weg, neu attachen ist ok — tmux-Session lebt ja weiter)

### 4d — Polish & Release

- [ ] Appearance: Terminal-Theme aus tws-Palette ableiten (default.toml → TerminiTerminalAppearance)
- [ ] Haptics, Pull-to-Refresh, Empty-States
- [ ] TestFlight via lokalem Build (kein EAS)

## Phase 5 — v1.1-Kandidaten (nicht planen, nur merken)

- Push-Notifications ("Agent done") — braucht APNs-Relay oder ntfy-artigen Dienst
- Pane-Preview im Dashboard (`capture_pane` existiert schon serverseitig)
- Android via ghostty-web-WebView hinter gleichem `TerminalView`-Interface
- Notes editieren
- tmux-Window/Pane-Navigation in der App (statt nur Session-Attach)

## Risiken

| Risiko | Wahrscheinlichkeit | Mitigation |
|---|---|---|
| Termini-API jung/instabil (7⭐) | mittel | Phase-0-Spike zuerst; Version pinnen; Fallback libghostty-spm direkt; Not-Fallback ghostty-web-WebView |
| libghostty-API-Bruch upstream | mittel | Prebuilt xcframework gepinnt, Upgrade bewusst |
| state.json-Races TUI↔Server | mittel | Atomic writes (Phase 1) + mtime-Reload beidseitig; Rest-Risiko akzeptiert (Last-Writer-Wins auf Collection-Ebene) |
| Mirror-Session-Leaks | niedrig | Cleanup on disconnect + Startup-Sweep + Heartbeat |
| RN-Bridge-Perf | eliminiert | WS nativ in Swift |

## Reihenfolge & Meilensteine

1. **M0**: Spike grün — Termini rendert pi/vim über WS-PTY sauber (Phase 0)
2. **M1**: `cargo test` grün nach Workspace-Split, TUI unverändert (Phase 1)
3. **M2**: `curl` + `wscat` gegen `tws-server` — State, CRUD, Events (Phase 2)
4. **M3**: Terminal im Spike-Client gegen echten `tws-server` (Phase 3)
5. **M4**: App: Host hinzufügen → Dashboard live → Session killen (Phase 4a/4b)
6. **M5**: App: Terminal-Session auf iPhone, Agent beobachten + eingreifen (Phase 4c)
7. **M6**: TestFlight-Build (Phase 4d)
