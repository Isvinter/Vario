# Vario – Gleitschirm-Flugcomputer in Rust

Private Flugcomputer-App (Vario-Ton, IGC-Tracking, Karte, Instrumente). Entwicklung zuerst
als Linux-Prototyp, Zielgerät ist ein iPhone 13 Pro Max. Kein zertifiziertes Instrument.
Die vollständige Aufgabenstellung steht in `STARTPROMPT.md`.

## Kommunikation

- Mit dem Nutzer Deutsch sprechen.
- Code, Kommentare, Commit-Messages und Doku im Code auf Englisch.
- Der Nutzer ist Rust-Einsteiger: Entscheidungen und nicht offensichtliche Rust-Konzepte
  (Ownership/Lifetimes, Traits, `Send`/`Sync`, Atomics, Feature-Flags …) kurz erklären.

## Phasen

Phasen strikt nacheinander, nach jeder Phase auf Freigabe warten.

| Phase | Inhalt | Status |
|-------|--------|--------|
| 0 | Prüfen und planen, `CLAUDE.md` | freigegeben |
| 1 | Workspace, `flight-core` (Sensor-Traits, IGC), `sensor-sim` | fertig, Freigabe offen |
| 2 | Vario-Filter, `vario-audio`, CLI auf Linux | offen |
| 2b | Früher iOS-Test am Mac: minimale Tauri-App, Ton + Sensoren bei gesperrtem Bildschirm | offen |
| 3 | App auf Linux (Instrumente, Einstellungen, Karte) | offen |
| 4 | Aufzeichnung, Flugliste | offen |
| 5 | iOS (Sensorbrücke, Hintergrund, Audio-Session) – auf dem Mac | offen |

Den Status hier nach jeder Freigabe aktualisieren.

## Architektur

Cargo-Workspace. Kernlogik ist strikt von Plattform und Oberfläche getrennt; die Oberfläche
muss austauschbar bleiben, ohne dass sich die Kern-Crates ändern.

```
Cargo.toml               workspace root
crates/
  flight-core/           platform independent: sensor traits (GPS, pressure, acceleration),
                         vario filter, track recording, IGC read/write, settings
  vario-audio/           tone synthesis (climb rate -> pitch/cadence); the synth itself is
                         pure and testable, the audio device backend sits behind a feature
  sensor-sim/            sensor source for development: IGC replay (real time or
                         accelerated) and synthetic profiles
  vario-cli/             small command line program (phase 2)
apps/
  vario-app/             UI app, as little logic as possible (phase 3+)
testdata/                real IGC files provided by the user
```

Regeln:

- `flight-core` hat keine UI-, OS- oder Audio-Abhängigkeiten und kein `std::time::Instant`/
  Systemuhr in der Logik: Zeit kommt immer als Zeitstempel mit den Sensorwerten herein.
  So laufen Simulator, beschleunigte Wiedergabe und Tests deterministisch.
- Sensoren werden nur über die Traits aus `flight-core` angesprochen. Simulator, Linux und
  iOS sind gleichberechtigte Implementierungen.
- Der Vario-Filter fusioniert Luftdruck und Beschleunigung (Kalman) und muss mit
  unterschiedlichen, unregelmäßigen Abtastraten umgehen (iOS-Barometer nur ca. 1 Hz).
- Alles Zeitkritische (Filter, Ton, Aufzeichnung) liegt in Rust und darf nicht von der
  Oberfläche abhängen – auf iOS ist die Oberfläche bei gesperrtem Bildschirm angehalten.
- Im Audio-Callback: keine Allokation, keine Locks, kein I/O. Parameter kommen lock-frei
  herein; Frequenz- und Lautstärkewechsel werden geglättet (kein Knacken).
- Spätere Features nicht verbauen: Flugbuch, Offline-Karten, Lufträume (openAIP),
  Start-/Landeerkennung, externes Bluetooth-Vario (wäre nur eine weitere Sensorquelle).

Technik (mit Phase 0 freigegeben):

- UI: Tauri 2 + MapLibre GL JS.
- Audio: `cpal` (ALSA/PipeWire auf Linux, CoreAudio auf iOS).
- iOS-Sensoren: kleine Swift-Brücke (CoreLocation, CoreMotion), die Messwerte per C-FFI
  direkt an Rust gibt – nicht über JavaScript.
- Kartenkacheln: Anbieter mit passender Nutzungsbedingung (Standard: OpenFreeMap),
  Kachel-URL konfigurierbar. Kein Massendownload, keine Nutzung von
  `tile.openstreetmap.org` als Bulk-Quelle. Attribution immer anzeigen.

## Arbeitsweise

- Kleine, überprüfbare Schritte, je ein Commit.
- Vor jedem Commit müssen sauber durchlaufen:
  ```
  cargo fmt --all -- --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```
- Kernlogik hat Tests. Filter-Tests prüfen gegen synthetische Profile mit bekanntem
  Steigwert; IGC-Tests prüfen den Roundtrip (lesen → schreiben → inhaltsgleich).
- Vor größeren Abhängigkeiten oder Abweichungen von der Architektur den Nutzer fragen.
- Klar benennen, was nur auf Linux bzw. nur im Simulator geprüft wurde. Keine Aussagen über
  das Verhalten auf dem echten Gerät, die dort nicht getestet wurden.
- Fehlen Testdaten: mit synthetischen Profilen arbeiten, bis IGC-Dateien in `testdata/`
  liegen.

## Nützliche Befehle

```
# Flug durch die simulierten Sensoren abspielen (Profile: demo, climb, thermal, sink)
cargo run -p sensor-sim --example replay -- --synthetic demo --speed 10
cargo run -p sensor-sim --example replay -- testdata/flug.igc --fast --out /tmp/kopie.igc
```

Rust ist per `rustup` im Home-Verzeichnis installiert (`~/.cargo/bin`).

## Fachliche Leitplanken

- Standardwerte Vario-Ton: Steigton ab +0,2 m/s, Sinkton ab −2,5 m/s, dazwischen Stille.
  Schwellen, Lautstärke und Dämpfung sind einstellbar und werden dauerhaft gespeichert.
- Tracking: 1 Hz, Ausgabe als IGC. Die Dateien sind nicht signiert (kein gültiger G-Record),
  also nicht für Wettbewerbe/Rekorde gedacht.
- Einheiten intern SI (m, m/s, Pa, s); Umrechnung nur in der Anzeige.
- iOS später: Standortberechtigung „Immer“, Background-Modes `location` und `audio`,
  Audio-Session-Kategorie `playback` (Ton trotz Stummschalter).
