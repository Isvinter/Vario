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
| 1 | Workspace, `flight-core` (Sensor-Traits, IGC), `sensor-sim` | freigegeben |
| 2 | Vario-Filter, `vario-audio`, CLI auf Linux | freigegeben |
| 2b | Früher iOS-Test am Mac: minimale Tauri-App, Ton + Sensoren bei gesperrtem Bildschirm | freigegeben (Tauri bleibt; Anruftest nicht bestanden → Phase 5; Akku offen; Ergebnisse: `docs/ios-test.md`) |
| 3 | App auf Linux (Instrumente, Einstellungen, Karte) | freigegeben (geprüft auf macOS und im iOS-Simulator; auf Linux seit Phase 2b nicht mehr geprüft) |
| 4 | Aufzeichnung, Flugliste | freigegeben (geprüft auf macOS und im iOS-Simulator; auf Linux nicht geprüft) |
| 5 | iOS (Sensorbrücke, Hintergrund, Audio-Session) – auf dem Mac | in Arbeit (Sensorbrücke, Hintergrund, Audio-Session-Kategorie schon aus 2b; zuerst Audio-Unterbrechungen) |
| 6 | Offline-Karten: Regionspakete der outdoor_app (`core-maps` per Git), Kachel-Schema `tiles://`, Topo-Stil | in Arbeit (Anzeige offline und Flugkarte auf dem iPhone geprüft; offen: Stromleitungen und Start-/Landeplätze über den Region-Builder der outdoor_app – dann Stromleitungen dünn gestrichelt, Seilbahnen dicker und durchgehend) |
| 6b | Fertiger Flugstil mit dem neuen Flug-Layer des Region-Builders (Stromleitungen dünn gestrichelt, Seilbahnen dicker und durchgehend, Start-/Landeplätze); Regionen per WLAN von der outdoor_app (Linux-Desktop, `core-share`) auf die Vario-App (iOS) übertragen | fertig, Freigabe ausstehend (Flugstil und WLAN-Übertragung auf dem iPhone geprüft: Aktualisieren, Abbrechen und Fortsetzen gegen `share-serve` am Mac; gegen die echte Freigabe der outdoor_app auf Linux noch nicht; Regionen ohne `flying` zeigen keine Seilbahnen mehr, also neu bauen; QR-Code zum Verbinden später) |
| 7 | Lufträume: Format einlesen (`flight-core`), Anzeige auf der Karte, später Warnungen; Datenquelle noch offen (openAIP oder OpenAir) | offen |

Den Status hier nach jeder Freigabe aktualisieren.

## Architektur

Cargo-Workspace. Kernlogik ist strikt von Plattform und Oberfläche getrennt; die Oberfläche
muss austauschbar bleiben, ohne dass sich die Kern-Crates ändern.

```
Cargo.toml               workspace root
crates/
  flight-core/           platform independent: sensor traits (GPS, pressure, acceleration),
                         vario filter, track recording, IGC read/write, flight summary,
                         settings
  vario-audio/           tone synthesis (climb rate -> pitch/cadence); the synth itself is
                         pure and testable, the audio device backend sits behind a feature
  sensor-sim/            sensor source for development: IGC replay (real time or
                         accelerated) and synthetic profiles
  vario-cli/             small command line program
  sensor-bridge/         C entry point (`vario_push_*`) through which native code pushes
                         sensor samples into a channel; the only crate allowed `unsafe`
  vario-engine/          background thread: channel -> filter -> tone, statistics, logs,
                         flight recording; also lists/loads/deletes recorded flights
apps/
  vario-app/
    src-tauri/           Tauri app: glue only (commands, audio thread, desktop simulator)
    plugins/vario-sensors/  Tauri plugin; `ios/Sources` holds the Swift sensor bridge
    ui/                  web UI (plain HTML/JS, no bundler); `vendor/` holds MapLibre GL JS 5
                         as unmodified files (version 6 needs ES modules and a worker file);
                         `map/fonts/` the map glyphs (Noto Sans, from the outdoor_app)
    src-tauri/map-styles/  offline map style (copy of the outdoor_app's topo-summer.json)
docs/ios-test.md         build and test instructions for the iOS test
testdata/                real IGC files provided by the user
```

Regeln:

- `flight-core` hat keine UI-, OS- oder Audio-Abhängigkeiten und kein `std::time::Instant`/
  Systemuhr in der Logik: Zeit kommt immer als Zeitstempel mit den Sensorwerten herein.
  So laufen Simulator, beschleunigte Wiedergabe und Tests deterministisch.
- Abfragbare Sensorquellen (Simulator) implementieren die Traits aus `flight-core`.
  Quellen, die von sich aus liefern (iOS-Callbacks), schieben `SensorEvent`s über
  `sensor-bridge` in einen Kanal. Die Engine liest immer aus diesem Kanal, auf dem Desktop
  speist ihn der Simulator. Hinter dem Kanal ist alles plattformgleich.
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
- Offline-Karten: Regionspakete (Ordner mit `manifest.json` und PMTiles) aus dem
  Region-Builder der outdoor_app (github.com/Isvinter/outdoor_app, eigenes Projekt des
  Nutzers), gelesen mit deren Crate `core-maps` (Git-Abhängigkeit, auf einen Commit
  festgelegt). Regionen liegen im Ordner `maps` neben `flights`; auf dem iPhone per Finder
  kopiert. Sind Regionen da, gewinnt die Offline-Karte.
- Online-Kartenkacheln: Anbieter mit passender Nutzungsbedingung (Standard: OpenFreeMap),
  Kachel-URL konfigurierbar. Kein Massendownload, keine Nutzung von
  `tile.openstreetmap.org` als Bulk-Quelle. Attribution immer anzeigen.

## Arbeitsweise

- Kleine, überprüfbare Schritte, je ein Commit.
- Vor jedem Commit müssen sauber durchlaufen:
  ```
  cargo fmt --all -- --check
  cargo clippy --all-targets -- -D warnings
  cargo test
  ```
  Ohne `--workspace` prüfen diese Befehle nur `crates/*` (die `default-members`). Die App
  unter `apps/` braucht die Tauri-Systempakete (`libwebkit2gtk-4.1-dev` usw.); sind sie
  installiert, zusätzlich mit `--workspace` prüfen.
- Exit-Status der Prüfungen nicht durch Pipes (`| tail`) verdecken.
- Kernlogik hat Tests. Filter-Tests prüfen gegen synthetische Profile mit bekanntem
  Steigwert; IGC-Tests prüfen den Roundtrip (lesen → schreiben → inhaltsgleich).
- Vor größeren Abhängigkeiten oder Abweichungen von der Architektur den Nutzer fragen.
- Klar benennen, was nur auf Linux bzw. nur im Simulator geprüft wurde. Keine Aussagen über
  das Verhalten auf dem echten Gerät, die dort nicht getestet wurden.
- Das Repository auf GitHub (`origin`) ist öffentlich: keine Wohnort-Koordinaten (GPS-Zeilen
  aus Sensorlogs von zu Hause entfernen), keine Zugangsdaten committen. Nur nach
  Rückfrage pushen.
- Fehlen Testdaten: mit synthetischen Profilen arbeiten, bis IGC-Dateien in `testdata/`
  liegen.

## Nützliche Befehle

```
# Flug durch die simulierten Sensoren abspielen (Profile: demo, climb, thermal, sink)
cargo run -p sensor-sim --example replay -- --synthetic demo --speed 10
cargo run -p sensor-sim --example replay -- testdata/flug.igc --fast --out /tmp/kopie.igc
```

```
# Vario hören: Flug durch Filter und Ton abspielen
cargo run -p vario-cli -- --synthetic demo
cargo run -p vario-cli -- testdata/flug.igc --speed 4 --climb-threshold 0.5 --volume 0.4
# Ton ohne Soundkarte in eine WAV-Datei schreiben
cargo run -p vario-cli -- --synthetic demo --noise --wav /tmp/demo.wav
```

```
# App auf dem Desktop starten (simulierter Flug)
cargo run -p vario-app
# Selbsttest: drückt selbst auf Start, öffnet die Karte, gibt Seiteninhalt, Anzahl der
# Track-Punkte und Skriptfehler aus und beendet sich
VARIO_SMOKE_TEST=1 cargo run -p vario-app
# Region per WLAN laden: Freigabe der outdoor_app nachstellen (deren share-serve, aus
# einem Checkout der outdoor_app; --seed-region verschiebt den Ordner, also eine Kopie
# angeben) und den Selbsttest mit leerem HOME dagegen laufen lassen, damit der echte
# maps-Ordner unberührt bleibt
cargo run --release -p core-share --bin share-serve -- --data /tmp/share --port 8765 --seed-region /tmp/kopie-der-region
cargo build -p vario-app && HOME=$(mktemp -d) VARIO_SMOKE_TEST=1 VARIO_SMOKE_SHARE=127.0.0.1:8765 target/debug/vario-app
```

Offline-Kartenregionen liegen im Ordner `maps` (Desktop: Datenordner der App; zum Testen
reicht dort ein Symlink auf eine Region der outdoor_app). Der Desktop-Simulator fliegt über
dem Brauneck, also in deren Testregion.

Flüge liegen als IGC-Dateien im Ordner `flights` (iOS: Dokumente-Ordner der App, in der
App „Dateien“ sichtbar; Desktop: Datenordner der App), Sensorlogs daneben in `logs`. Jeder
Lauf von Start bis Stopp ist ein Flug; die Datei wird alle 10 s vollständig neu geschrieben.
Der Selbsttest schreibt seine Flüge in einen temporären Ordner.

Einstellungen liegen als Textdatei (`settings.txt`, Kartenstil in `map-style.txt`) im
Konfigurationsordner der App; das Format definiert `flight-core`.

Prüfen auf dem iPhone am Kabel: Gerätelog live mit `idevicesyslog` (Meldungen aus Swift
beginnen mit `vario:`), Absturz nachstellen mit `devicectl … process terminate --kill`,
Regionen kopieren mit `devicectl … copy to`; Befehle in `docs/ios-test.md`, Abschnitt 5.

Rust ist per `rustup` im Home-Verzeichnis installiert (`~/.cargo/bin`). Auf dem Mac läuft
die Shell von Claude Code unter Rosetta; die Toolchain ist trotzdem nativ arm64
(`aarch64-apple-darwin`). `timeout` gibt es auf macOS nicht.

## Fachliche Leitplanken

- Standardwerte Vario-Ton: Steigton ab +0,2 m/s, Sinkton ab −2,5 m/s, dazwischen Stille.
  Schwellen, Lautstärke und Dämpfung sind einstellbar und werden dauerhaft gespeichert.
- Tracking: 1 Hz, Ausgabe als IGC. Die Dateien sind nicht signiert (kein gültiger G-Record),
  also nicht für Wettbewerbe/Rekorde gedacht.
- Einheiten intern SI (m, m/s, Pa, s); Umrechnung nur in der Anzeige.
- iOS später: Standortberechtigung „Immer“, Background-Modes `location` und `audio`,
  Audio-Session-Kategorie `playback` (Ton trotz Stummschalter) mit `mixWithOthers`: Der Ton
  mischt sich mit anderen Apps; nur so darf iOS ihn nach einem Anruf im Hintergrund
  wieder starten.
