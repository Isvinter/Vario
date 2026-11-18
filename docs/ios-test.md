# Früher iOS-Test (Phase 2b)

Ziel: herausfinden, ob Tauri auf dem iPhone für den Flugcomputer trägt, **bevor** die
Oberfläche gebaut wird. Die Test-App spielt den Vario-Ton aus den echten Sensoren, zeigt,
wie regelmäßig jeder Sensor liefert, und schreibt alles in Logdateien.

**Stand (17.11.2026):** Die App läuft auf dem **iPhone 13 Pro Max (iOS 27.2)**. Die Teile
A und B des Testablaufs sind bestanden, Teil C ist bis auf den Akku durchgeführt: GPS
draußen in Ordnung, der Anruftest ist **nicht bestanden** (Ursache bekannt, siehe unten).
Phase 2b ist damit freigegeben; der Anruf-Fehler wird in Phase 5 behoben.

**Auf dem iPhone geprüft (im Gebäude, am Kabel):**

- Build, Signierung und Installation mit kostenloser Apple-ID („Personal Team“)
  funktionieren, auch mit den Hintergrund-Modi `audio` und `location`.
- Teil A: Start, Berechtigungen, Testtöne, Ton trotz Stummschalter und das Vorzeichen der
  Beschleunigung (Anheben schlägt zuerst positiv aus) stimmen laut Nutzer.
- Teil B, je etwa 10 Minuten, ausgewertet aus den Rohdaten in `testdata/ios/`
  (`sensors-20261111-111622.csv`, 26 Minuten am Stück; die GPS-Zeilen sind entfernt,
  weil das Repository öffentlich ist):

  | Abschnitt | Beschleunigung | Luftdruck |
  |---|---|---|
  | gesperrt, mit Testton | 49,7 Hz, längste Pause 0,020 s | 0,94 Hz, längste Pause 1,07 s |
  | gesperrt, ohne Ton | 49,7 Hz, längste Pause 0,035 s | 0,94 Hz, längste Pause 1,07 s |
  | andere App im Vordergrund | 49,7 Hz, längste Pause 0,020 s | 0,94 Hz, längste Pause 1,07 s |

  Keine einzige Lücke; der Testton lief laut Nutzer gesperrt durch. Der Steigwert steht
  in Ruhe bei etwa ±0,1 m/s.

**Teil C (draußen, ohne Kabel, 15. und 17.11.2026):** vier Läufe von 13 bis 22 Minuten
mit dem Fahrrad in der Stadt und zu Fuß, nicht im Flug. Ausgewertet aus den Rohdaten
(nicht im Repository, weil sie Positionen enthalten):

- Beschleunigung und Luftdruck liefen in allen Läufen ohne Lücke (49,7 Hz bzw. 0,94 Hz),
  auch während eines Anrufs.
- GPS liefert draußen 1 Hz mit 3D-Fix. In Gebäuden fällt es bis zu drei Minuten aus;
  iOS liefert dann grobe Positionen ohne Höhe, die Hunderte Meter springen können.
- Jeder Luftdruckwert erreicht Rust etwa **2,7 s nach seinem Zeitstempel**. Filter und
  Aufzeichnung verrechnen ihn deshalb zu seinem Messzeitpunkt (seit `b6af3fe` bzw.
  `5319e20`); vorher fehlte die Druckhöhe in den IGC-Dateien.
- Rauschen der Druckhöhe: etwa 4 cm bei ruhigem Wetter, etwa 30 cm (Ausreißer bis 3 m)
  bei Sturm zwischen Gebäuden.
- **Anruf:** Danach kam der Ton nicht zurück. Sensoren, Filter und Aufzeichnung liefen
  weiter. Ursache: Die App reagierte noch nicht auf Unterbrechungen der Audio-Session.
  **Behoben in Phase 5** (`e0e4f24` und Folge-Commit): Die App aktiviert die Session nach
  einer Unterbrechung wieder und öffnet die Tonausgabe neu. Im Hintergrund klappte das
  erst mit der Option `mixWithOthers`; ohne sie lehnte iOS ab („can't stop others
  because it is in the background and not the NowPlaying app“, im Gerätelog). Auf dem
  iPhone mit Wecker bei gesperrtem Bildschirm geprüft: Der Wecker schaltet den Vario-Ton
  ganz stumm, nach dem Stoppen ist er sofort wieder da. Ob ein Anruf sich genauso
  verhält, ist nach der Korrektur nicht eigens geprüft.
- **Akku:** Die Anzeige stand vor und nach einem 13-Minuten-Lauf auf 100 %; für eine
  Aussage braucht es einen längeren Lauf.

**Noch offen:** Akkuverbrauch über eine Stunde und mehr, Ton nach einem echten Anruf
(nach der Korrektur), Vario-Gefühl bei echtem Höhenunterschied im Flug.

**Bewertung:** Tauri trägt für den Hintergrundbetrieb, die Oberfläche kann wie geplant
mit Tauri gebaut werden.

**Im Simulator geprüft (MacBook Air M3, macOS 27.2.1, Xcode 27.2, iOS-SDK 27.2,
Simulator „iPhone 18 Pro Max“):**

- `cargo tauri ios init` und `cargo tauri ios build --debug --target aarch64-sim` laufen
  ohne Fehler durch; der Swift-Code übersetzt ohne Änderung. Das erzeugte Xcode-Projekt
  liegt unter `apps/vario-app/src-tauri/gen/apple` im Repository.
- `Info.ios.plist` wird übernommen: Hintergrund-Modi `audio` und `location` sowie die
  Berechtigungstexte stehen in der Info.plist der gebauten App.
- Die App startet, „Start“ funktioniert (Plugin-Aufruf Rust → Swift), die Audio-Session
  meldet `ok`, die Tonausgabe öffnet mit 48000 Hz und 5,3 ms Puffer.
- Simulierte GPS-Positionen kommen mit etwa 1 Hz über die C-Funktionen in Rust an und
  landen in den Logdateien im Dokumente-Ordner.
- Der Rust-Code übersetzt auch für das Gerät (`cargo check --target aarch64-apple-ios`).

**Im Simulator nicht prüfbar:** Barometer und Beschleunigung (gibt es dort nicht, die App
zeigt „NEIN“), hörbarer Ton, Stummschalter, Verhalten bei gesperrtem Bildschirm, Akku.
Der Build für das Gerät samt Signierung ist ebenfalls noch nicht gelaufen.

Außerdem auf dem Mac geprüft: `cargo fmt`, `clippy` und `test` laufen für den ganzen
Workspace sauber durch (93 Tests); die Desktop-App besteht den Selbsttest
(`VARIO_SMOKE_TEST=1`) mit Ton über CoreAudio.

### Simulator-Test wiederholen

```sh
cd apps/vario-app/src-tauri
cargo tauri ios build --debug --target aarch64-sim --ci
xcrun simctl boot "iPhone 18 Pro Max"
xcrun simctl install booted gen/apple/build/arm64-sim/Vario.app
xcrun simctl privacy booted grant location-always de.vario.prototyp.flugcomputer
xcrun simctl location booted start --speed=15 47.60,11.00 47.62,11.03
# Selbsttest: drückt selbst auf Start und beendet die App nach 14 s
SIMCTL_CHILD_VARIO_SMOKE_TEST=1 xcrun simctl launch booted de.vario.prototyp.flugcomputer
```

## Die Fragen, die der Test beantworten soll

1. Baut und startet die Tauri-App auf dem iPhone?
2. Kommen GPS, Luftdruck und Beschleunigung in Rust an, und mit welcher Rate?
3. Läuft der Ton bei **gesperrtem Bildschirm** weiter?
4. Liefern die Sensoren bei gesperrtem Bildschirm lückenlos weiter?
5. Ist der Ton trotz **Stummschalter** hörbar?
6. Stimmt das Vorzeichen der Beschleunigung, und fühlt sich das Vario brauchbar an?

## 1. Mac vorbereiten

Xcode ist Pflicht: Die Kommandozeilenwerkzeuge allein bringen kein iOS-SDK mit. Bei der
Installation beachten:

- Platz: Xcode samt iOS-Plattform belegt rund 20 GB, während der Installation mehr.
  Vorher für mindestens 30–40 GB freien Platz sorgen (`target/` des Projekts kann mit
  `cargo clean` jederzeit weg, das sind einige GB).
- Version: Die iOS-Version des iPhones muss vom SDK der Xcode-Version unterstützt sein.
  macOS 15.5 trägt Xcode 16.4 (bis iOS 18.x). Läuft auf dem iPhone bereits iOS 26,
  ist Xcode 26 nötig, das wiederum ein neueres macOS verlangt – dann zuerst macOS
  aktualisieren.
- Nach der Installation Xcode einmal starten (Lizenz, Zusatzkomponenten, iOS-Plattform
  herunterladen) und die Kommandozeile darauf umstellen:
  ```sh
  sudo xcode-select -s /Applications/Xcode.app/Contents/Developer
  xcrun --sdk iphoneos --show-sdk-path     # muss einen Pfad liefern
  ```

Hinweis: Die Shell, in der Claude Code auf diesem Mac läuft, läuft unter Rosetta
(`arch` meldet `i386`). Rust wurde deshalb ausdrücklich als arm64-Toolchain installiert
(`rustup-init --default-host aarch64-apple-darwin`); `rustup show` muss
`aarch64-apple-darwin` als Host zeigen. Cargo liegt in `~/.cargo/bin`; damit es in jeder
Shell gefunden wird, gehört `. "$HOME/.cargo/env"` in die `~/.zshrc`.

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

Das Projekt liegt auf GitHub (`origin`) und wird von dort auf den Mac geklont.

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

Eine kostenlose Apple-ID („Personal Team“) reicht, auch für die Hintergrund-Modi
(geprüft). Die installierte App läuft damit nach 7 Tagen ab und muss dann neu
installiert werden (vom Nutzer geprüft); außerdem sind höchstens 3 selbst signierte
Apps gleichzeitig installierbar. Mit dem bezahlten Apple Developer Program
(99 $/Jahr) gilt die Signatur ein Jahr, und die Grenze von 3 Apps entfällt.

Was beim ersten Mal zusätzlich nötig war:

- In Xcode unter *Settings → Accounts → Manage Certificates…* ein Zertifikat
  „Apple Development“ anlegen. Die Team-ID steht danach im Zertifikat (Feld OU).
- Das iPhone muss beim Team registriert sein, sonst stellt Apple kein Profil aus
  („Your team has no devices“). `cargo tauri ios build` registriert es nicht selbst;
  einmalig erledigt das dieser Aufruf (er scheitert danach am Tauri-Build-Skript, das
  ist dann egal – das Profil existiert):
  ```sh
  cd apps/vario-app/src-tauri/gen/apple
  xcodebuild -project vario-app.xcodeproj -scheme vario-app_iOS -configuration debug \
    -destination 'id=<UDID aus: xcrun devicectl list devices>' \
    -allowProvisioningUpdates -allowProvisioningDeviceRegistration build
  ```

## 3. Bauen und installieren

iPhone per Kabel anschließen und entsperren.

```sh
cd apps/vario-app/src-tauri
cargo tauri ios init        # nur nötig, falls gen/apple fehlt (liegt im Repository)
cargo tauri ios run         # baut, installiert und startet auf dem Gerät
```

Geprüft ist dieser Weg (statt `ios run`):

```sh
cargo tauri ios build --debug --target aarch64 --ci
xcrun devicectl device install app --device <UDID> gen/apple/build/arm64/Vario.ipa
```

`ios run` bettet die Oberfläche in die App ein. Die App läuft danach auch ohne
Verbindung zum Mac, was für den Test im Freien nötig ist. (`cargo tauri ios dev` lädt
die Oberfläche dagegen vom Mac und ist nur am Schreibtisch brauchbar.)

`Info.ios.plist` wird erst beim Bauen eingemischt: Die Hintergrund-Modi und die
`NS…UsageDescription`-Texte stehen in der Info.plist der gebauten App, nicht in
`gen/apple/vario-app_iOS/Info.plist` (im Simulator-Build geprüft).

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

Per AirDrop auf den Mac schicken und in `testdata/ios/` im Projekt ablegen. **Vor einem
Commit die GPS-Zeilen entfernen** (`grep -v "^g," …`), wenn die Aufnahme zu Hause
entstand – das Repository ist öffentlich. Am Kabel geht es direkt:

```sh
xcrun devicectl device copy from --device <UDID> --domain-type appDataContainer \
  --domain-identifier de.vario.prototyp.flugcomputer \
  --source Documents/logs --destination testdata/ios
```

Die Rohdaten sind die Grundlage, um den Vario-Filter auf echte Sensoren abzustimmen.

### Weitere Prüfwege am Kabel

Die UDID liefert `xcrun devicectl list devices`.

- **Gerätelog live mitschneiden** (die Meldungen aus Swift beginnen mit `vario:`; das Log
  lässt sich nur live lesen, nicht nachträglich): `idevicesyslog` aus Homebrew
  (`libimobiledevice`):
  ```sh
  idevicesyslog -u <UDID> | grep --line-buffered "vario:"
  ```
  Für Gründe, die iOS selbst meldet (z. B. warum die Audio-Session nicht aktiviert
  wird), ungefiltert mitschneiden und danach nach `Vario(` suchen; das ungefilterte Log
  wird schnell sehr groß.
- **Absturz nachstellen** (die App bekommt keine Gelegenheit mehr zu speichern, wie bei
  Speichermangel oder leerem Akku):
  ```sh
  xcrun devicectl device info processes --device <UDID> | grep Vario.app
  xcrun devicectl device process terminate --device <UDID> --pid <PID> --kill
  ```
  Wegwischen im App-Umschalter ist dagegen ein geordnetes Ende (die App speichert).
- **Kartenregion aufs iPhone kopieren** (statt per Finder):
  ```sh
  xcrun devicectl device copy to --device <UDID> --domain-type appDataContainer \
    --domain-identifier de.vario.prototyp.flugcomputer \
    --source <region-ordner>/ --destination Documents/maps/<region-id>
  ```

### Regionen per WLAN vom Desktop laden

Die Freigabe der outdoor_app (Linux-Desktop) liefert die Regionen, die Vario-App lädt sie
unter Einstellungen → Offline-Karten → „Desktop im WLAN“. Ohne Linux-Rechner tut es
auch `share-serve` aus der outdoor_app auf dem Mac (Befehl in `CLAUDE.md`); dann die
WLAN-Adresse des Macs eintragen (`ipconfig getifaddr en0`), Port 8765.

Im Simulator geprüft (Download, Anzeige auf der Karte); auf dem iPhone zu prüfen:

1. Adresse eintragen, „Verbinden“: iOS fragt einmal nach dem **lokalen Netzwerk**
   („Lädt Kartenregionen …“) – erlauben. Abgelehnt: in den iOS-Einstellungen unter
   Vario → Lokales Netzwerk einschalten.
2. „Laden“: Fortschrittsbalken läuft, danach „… ist geladen.“ und die Karte zeigt die
   Region.
3. Während des Ladens „Abbrechen“, dann wieder „Laden“: setzt an derselben Stelle fort.
4. Während des Ladens den Bildschirm sperren und wieder entsperren: Lädt es weiter, oder
   kommt „Verbindung zum Desktop unterbrochen“? Danach „Laden“ setzt fort.
5. Ist auf dem Desktop eine neuere Version gebaut: „Update auf dem Desktop“ und
   „Aktualisieren“.
6. „Löschen“ (zweimal tippen): Region verschwindet aus Liste, Karte und App „Dateien“.

## 6. Bewertung

| Ergebnis | Folge |
|---|---|
| Ton und Sensoren laufen gesperrt lückenlos | Tauri bleibt, weiter mit Phase 3. |
| Sensoren laufen nur, solange ein Ton spielt | Lösbar (z. B. unhörbarer Dauerton im Flug), weiter mit Phase 3. |
| App wird im Hintergrund angehalten und lässt sich nicht am Laufen halten | Oberfläche auf SwiftUI umstellen; Rust-Kern, `sensor-bridge` und der Swift-Sensorcode bleiben. |
| Barometer oder Beschleunigung unbrauchbar | Filter anhand der Rohdaten neu abstimmen; notfalls externes Bluetooth-Vario vorziehen. |
