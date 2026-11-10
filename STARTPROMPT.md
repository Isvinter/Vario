# Startprompt: Gleitschirm-Flugcomputer in Rust

## Ziel

Ich möchte eine eigene Flugcomputer-App fürs Gleitschirmfliegen bauen. Sie soll am Ende auf meinem iPhone 13 Pro Max laufen, wird aber zuerst auf meinem Linux-Rechner als Prototyp entwickelt. Die App ist nur für mich gedacht, eine Veröffentlichung im App Store ist vorerst kein Ziel.

Mir ist wichtig, dass die App performant ist und als nativ kompiliertes Binary läuft. Alles Zeitkritische (Sensorfilter, Vario-Ton, Track-Aufzeichnung) soll deshalb in Rust liegen.

## Über mich und meine Umgebung

- Ich habe gerade meinen A-Schein gemacht, bin also Flugeinsteiger. Die App ist ein Hilfsmittel und kein zertifiziertes Instrument.
- Rust-Erfahrung: [BITTE EINTRAGEN: Beginner mit etwas Grundlagen. Erkläre Entscheidungen und nicht offensichtliche Rust-Konzepte so, dass ich sie nachvollziehen kann.
- Entwicklungsrechner: Linux (Ubuntu).
- Für den späteren iOS-Build habe ich ein MacBook Air M3 mit Xcode zur Verfügung.
- Sprich Deutsch mit mir. Code, Kommentare und Commit-Messages bitte auf Englisch.

## Funktionsumfang der ersten Version

1. **Varioton**, einstellbar:
   - Steigton ab einer einstellbaren Schwelle (Standard +0,2 m/s): Pieptöne, deren Tonhöhe und Taktfrequenz mit dem Steigen zunehmen.
   - Sinkton ab einer einstellbaren Schwelle (Standard −2,5 m/s): tiefer Dauerton.
   - Dazwischen Stille.
   - Einstellbar sind außerdem Lautstärke und Dämpfung (Mittelungszeit des Varios).
   - Die Einstellungen werden dauerhaft gespeichert.
2. **Tracking:** GPS-Position und Höhe mit 1 Hz aufzeichnen und als IGC-Datei speichern.
3. **Karte:** Open-Source-Karten mit eigener Position und geflogenem Track.
4. **Anzeige:** Steigwert, Höhe, Geschwindigkeit über Grund und Kurs, gut ablesbar bei Sonne.

Später geplant, jetzt noch nicht bauen, aber in der Architektur nicht verbauen: Flugbuch mit Statistik, Offline-Karten, Lufträume (openAIP), automatische Start- und Landeerkennung, externes Bluetooth-Vario.

## Architektur-Vorgaben

Lege ein Cargo-Workspace an und trenne Kernlogik strikt von Plattform und Oberfläche:

- `flight-core`: plattformunabhängig, ohne UI- und OS-Abhängigkeiten. Enthält die Sensor-Schnittstellen (Traits für GPS, Luftdruck, Beschleunigung), den Vario-Filter, die Track-Aufzeichnung, IGC lesen und schreiben sowie die Einstellungen.
- `vario-audio`: Tonerzeugung in Rust. Bildet den Steigwert auf Tonhöhe und Takt ab.
- `sensor-sim`: Sensorquelle für die Entwicklung, die IGC-Dateien in Echtzeit oder beschleunigt abspielt und zusätzlich synthetische Profile erzeugen kann (z. B. gleichmäßiges Steigen, Thermikkreise, starkes Sinken).
- App mit Oberfläche: bindet die Crates ein und enthält selbst möglichst wenig Logik.

Mein bisheriger Favorit für die Oberfläche ist Tauri 2 mit MapLibre GL JS für die Karte, weil derselbe Code auf Linux und iOS läuft. Das ist aber nicht entschieden, siehe Phase 0. Die Oberfläche muss austauschbar bleiben, ohne dass sich die Kern-Crates ändern.

## Wichtige fachliche Punkte

- **Vario auf dem iPhone:** iOS liefert Barometerwerte nach meinem Kenntnisstand nur etwa einmal pro Sekunde. Das ist für ein Vario zu träge. Plane deshalb von Anfang an eine Sensorfusion aus Luftdruck und Beschleunigung (z. B. Kalman-Filter). Der Filter muss mit unterschiedlichen Abtastraten der beiden Quellen umgehen können.
- **Ton-Latenz:** Der Ton darf dem Steigwert nicht spürbar hinterherhinken und darf beim Wechsel von Tonhöhe oder Takt nicht knacken.
- **iOS-Besonderheiten für später:** Tracking und Ton müssen bei gesperrtem Bildschirm weiterlaufen (Standortberechtigung "Immer", Background-Modes für Location und Audio). Der Ton soll auch bei aktiviertem Stummschalter hörbar sein.
- **Kartenkacheln:** Nutzungsbedingungen der Kachelanbieter einhalten, kein Massendownload von den OpenStreetMap-Servern.

## Vorgehen in Phasen

Arbeite die Phasen nacheinander ab und warte nach jeder Phase auf meine Freigabe.

**Phase 0: Prüfen und planen (noch kein Code).**
Prüfe anhand der aktuellen Dokumentation, wie gut die infrage kommenden Bausteine heute iOS unterstützen: das UI-Framework (Tauri 2 gegen mindestens eine Alternative, z. B. Slint oder eine SwiftUI-Hülle mit Rust-Kern über UniFFI), die Audio-Ausgabe aus Rust auf iOS und der Zugriff auf GPS, Barometer und Beschleunigungssensor. Gib mir danach eine Empfehlung mit Begründung, den Risiken und einem Vorschlag für die Projektstruktur. Lege außerdem eine `CLAUDE.md` mit den Projektkonventionen an.

**Phase 1: Kern und Simulator.**
Workspace, `flight-core` mit Sensor-Traits, IGC lesen und schreiben, `sensor-sim`. Fertig, wenn eine IGC-Datei eingelesen, abgespielt und wieder identisch im Inhalt geschrieben werden kann und Unit-Tests das belegen.

**Phase 2: Vario und Ton auf Linux.**
Vario-Filter und `vario-audio`, dazu ein kleines Kommandozeilenprogramm. Fertig, wenn ich eine IGC-Datei abspielen und das Vario dazu hören kann, die Schwellen und die Lautstärke per Parameter ändern kann und Tests den Filter gegen synthetische Profile mit bekanntem Steigwert prüfen.

**Phase 3: App auf Linux.**
Oberfläche mit Instrumentenanzeige, Einstellungsseite und Karte mit Position und Track, gespeist aus dem Simulator. Fertig, wenn ein simulierter Flug vollständig in der App mitläuft.

**Phase 4: Aufzeichnung.**
Flug starten und stoppen, IGC-Datei speichern, gespeicherte Flüge auflisten und auf der Karte anzeigen.

**Phase 5: iOS.**
Sensorbrücke zu CoreLocation und CoreMotion, Hintergrundbetrieb, Audio-Session, Build und Installation auf dem iPhone. Für diese Phase wechsle ich auf den Mac.

## Arbeitsweise

- Kleine, überprüfbare Schritte mit jeweils einem Commit.
- Für die Kernlogik gibt es Tests. `cargo test`, `cargo clippy` und `cargo fmt` müssen vor jedem Commit sauber durchlaufen.
- Frage mich, bevor du größere Abhängigkeiten aufnimmst oder von den Architektur-Vorgaben abweichst.
- Sage klar, was du nur auf Linux oder nur im Simulator geprüft hast. Behaupte nichts über das Verhalten auf dem echten Gerät, das nicht dort getestet wurde.
- Wenn dir Testdaten fehlen: Ich lege echte IGC-Dateien in `testdata/` ab. Bis dahin arbeite mit den synthetischen Profilen.

Beginne jetzt mit Phase 0.
