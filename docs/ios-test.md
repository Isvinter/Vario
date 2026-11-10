# Früher iOS-Test (Phase 2b)

Ziel: herausfinden, ob Tauri auf dem iPhone für den Flugcomputer trägt, **bevor** die
Oberfläche gebaut wird. Die Test-App spielt den Vario-Ton aus den echten Sensoren, zeigt,
wie regelmäßig jeder Sensor liefert, und schreibt alles in Logdateien.

**Stand:** Der gesamte iOS-Teil (Swift-Code, Tauri-Konfiguration, diese Anleitung) wurde
auf Linux geschrieben und ist **weder kompiliert noch auf einem Gerät gelaufen**. Rechne
damit, dass beim ersten Build Fehler zu beheben sind. Auf Linux getestet sind die
Rust-Crates darunter (`sensor-bridge`, `vario-engine`, Filter, Ton) sowie die App selbst
mit simuliertem Flug: Oberfläche, Start/Stopp, Tonausgabe und Logdateien.

## Die Fragen, die der Test beantworten soll

1. Baut und startet die Tauri-App auf dem iPhone?
2. Kommen GPS, Luftdruck und Beschleunigung in Rust an, und mit welcher Rate?
3. Läuft der Ton bei **gesperrtem Bildschirm** weiter?
4. Liefern die Sensoren bei gesperrtem Bildschirm lückenlos weiter?
5. Ist der Ton trotz **Stummschalter** hörbar?
6. Stimmt das Vorzeichen der Beschleunigung, und fühlt sich das Vario brauchbar an?

## 1. Mac vorbereiten

```sh
# Xcode aus dem App Store installieren und einmal starten (Lizenz, Zusatzkomponenten).
xcode-select --install            # falls die Kommandozeilenwerkzeuge fehlen

# Rust
curl --proto '=https' --tlsv1.2 https://sh.rustup.rs -sSf | sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim

# Von Tauri für iOS verlangt
brew install cocoapods

# Tauri-Kommandozeile (dauert einige Minuten)
cargo install tauri-cli --version "^2" --locked
```

Das Projekt muss auf den Mac. Es gibt noch kein Git-Remote; am einfachsten ist ein
privates Repository (GitHub o. ä.) oder ein Bundle:

```sh
# auf Linux
git bundle create vario.bundle --all
# auf dem Mac
git clone vario.bundle Vario
```

## 2. Signierung

- In Xcode unter *Settings → Accounts* die Apple-ID hinzufügen.
- Die Team-ID steht unter <https://developer.apple.com/account> (Membership) bzw. in
  Xcode beim Account. Vor dem Bauen setzen:
  ```sh
  export APPLE_DEVELOPMENT_TEAM=XXXXXXXXXX
  ```
- Die Bundle-ID steht in `apps/vario-app/src-tauri/tauri.conf.json` (`identifier`).
  Meldet Xcode, dass sie vergeben ist, dort eine eigene eintragen.
- Auf dem iPhone: *Einstellungen → Datenschutz & Sicherheit → Entwicklermodus*
  einschalten. Nach der ersten Installation dem Entwicklerzertifikat vertrauen
  (*Allgemein → VPN & Geräteverwaltung*).

**Ungeklärt:** Die Tauri-Dokumentation nennt das kostenpflichtige Apple Developer
Program als Voraussetzung. Ob eine kostenlose Apple-ID („Personal Team“) hier reicht
und ob damit die Hintergrund-Modi funktionieren, ist nicht geprüft. Mit kostenloser
Apple-ID läuft eine installierte App nach meinem Kenntnisstand nach 7 Tagen ab.

## 3. Bauen und installieren

iPhone per Kabel anschließen und entsperren.

```sh
cd apps/vario-app/src-tauri
cargo tauri ios init        # erzeugt das Xcode-Projekt unter gen/apple
cargo tauri ios run         # baut, installiert und startet auf dem Gerät
```

`ios run` bettet die Oberfläche in die App ein. Die App läuft danach auch ohne
Verbindung zum Mac, was für den Test im Freien nötig ist. (`cargo tauri ios dev` lädt
die Oberfläche dagegen vom Mac und ist nur am Schreibtisch brauchbar.)

Prüfen, ob `Info.ios.plist` übernommen wurde: In `gen/apple/vario-app_iOS/Info.plist`
müssen `UIBackgroundModes` mit `audio` und `location` sowie die drei
`NS…UsageDescription`-Texte stehen. Fehlen die Hintergrund-Modi, stürzt die App beim
Tippen auf „Start“ ab.

### Wahrscheinliche Stolperstellen beim ersten Build

| Symptom | Vermutliche Ursache |
|---|---|
| Linker: `Undefined symbol: _vario_push_accel` | Die C-Funktionen aus `sensor-bridge` landen nicht in der statischen Bibliothek. In `apps/vario-app/src-tauri/src/lib.rs` wird `sensor_bridge` verwendet; ggf. muss der Linker die Symbole ausdrücklich behalten. |
| Swift-Fehler in `VarioSensorsPlugin.swift` | Der Code ist unkompiliert. Typische Kandidaten: der Rückgabetyp `JSObject`, `invoke.resolve()`, die `@_silgen_name`-Deklarationen. |
| Absturz bei „Start“ | Hintergrund-Modus `location` fehlt in der Info.plist (siehe oben). |
| Kein Ton, `Ton: no audio: …` in der App | `cpal` konnte die Ausgabe nicht öffnen; Meldung notieren. |
| App hängt bei „Start“ | Der Plugin-Aufruf wartet auf den Haupt-Thread. `start` muss ein `async`-Kommando bleiben. |

## 4. Testablauf

Nach jedem Schritt das Ergebnis notieren. Die Tabelle „Sensoren“ in der App zeigt je
Sensor Anzahl, Rate, Alter des letzten Werts und die längste Pause seit dem Start;
auffällige Werte sind rot.

### A. Am Schreibtisch

1. App öffnen, **Start** tippen. Standort erlauben, bei der zweiten Nachfrage
   **„Immer erlauben“** wählen (sonst später in *Einstellungen → Vario → Standort*).
   Bewegungs-/Fitnessdaten erlauben.
2. Die Infozeile unter den Knöpfen notieren: Barometer ja/nein, Bewegung ja/nein,
   Standort-Status, Audio-Session, Ton.
3. Nach 30 Sekunden die Raten ablesen. Erwartet: Beschleunigung etwa 50 Hz, Luftdruck
   etwa 1 Hz, GPS etwa 1 Hz (im Gebäude evtl. kein GPS).
4. **Testton Steigen** und **Testton Sinken** tippen: Pieptöne bzw. tiefer Dauerton.
5. **Stummschalter** einschalten: Der Ton muss hörbar bleiben.
6. **Vorzeichen:** Testton aus. Das iPhone ruhig halten, dann zügig etwa einen Meter
   anheben. Die Anzeige muss **zuerst positiv** ausschlagen. Schlägt sie zuerst negativ
   aus, ist das Vorzeichen in `VarioSensorsPlugin.swift` (`startMotion`) falsch.
7. Treppe oder Aufzug: Steigwert und Ton plausibel? Reagiert der Ton sofort?

### B. Gesperrter Bildschirm (der eigentliche Test)

1. **Testton Steigen** einschalten, Uhrzeit notieren, Bildschirm sperren.
2. Zehn Minuten liegen lassen. Läuft der Ton die ganze Zeit durch?
3. Entsperren. In der Tabelle „Sensoren“ die Spalte **längste Pause** ablesen und im
   Protokoll nach Zeilen mit großem `gap` oder Raten deutlich unter dem Sollwert suchen.
   Fehlen ganze 10-Sekunden-Zeilen, war die App angehalten.
4. Wiederholen **ohne** Testton (Vario still, also keine Tonausgabe): Liefern die
   Sensoren trotzdem weiter? Das entspricht ruhigem Gleiten im Flug.
5. Wiederholen mit einer anderen App im Vordergrund statt gesperrtem Bildschirm.

### C. Draußen

1. 20 bis 30 Minuten zu Fuß, mit dem Rad oder im Auto, möglichst mit Höhenunterschied,
   Bildschirm überwiegend gesperrt.
2. Akkustand vorher und nachher notieren.
3. Ein Anruf oder eine Sprachnachricht zwischendurch: Kommt der Ton danach zurück?

## 5. Logs auf den Rechner holen

Die App schreibt in ihren Dokumente-Ordner, sichtbar in der App **Dateien** unter
*Auf meinem iPhone → Vario → logs*:

- `summary-<Zeit>.txt`: eine Zeile alle 10 Sekunden mit Raten und längster Pause.
- `sensors-<Zeit>.csv`: jeder einzelne Sensorwert (etwa 5 MB pro Stunde).

Per AirDrop auf den Mac schicken und in `testdata/ios/` im Projekt ablegen. Die
Rohdaten sind die Grundlage, um den Vario-Filter auf echte Sensoren abzustimmen.

## 6. Bewertung

| Ergebnis | Folge |
|---|---|
| Ton und Sensoren laufen gesperrt lückenlos | Tauri bleibt, weiter mit Phase 3. |
| Sensoren laufen nur, solange ein Ton spielt | Lösbar (z. B. unhörbarer Dauerton im Flug), weiter mit Phase 3. |
| App wird im Hintergrund angehalten und lässt sich nicht am Laufen halten | Oberfläche auf SwiftUI umstellen; Rust-Kern, `sensor-bridge` und der Swift-Sensorcode bleiben. |
| Barometer oder Beschleunigung unbrauchbar | Filter anhand der Rohdaten neu abstimmen; notfalls externes Bluetooth-Vario vorziehen. |
