// The user interface. It only displays what Rust reports and passes on what the user
// taps; filter, tone and track live in Rust and keep running while this page is paused
// (screen locked).

"use strict";

// Collected for the smoke test (see smoke.rs), which reports them.
window.scriptErrors = [];
window.addEventListener("error", (event) => window.scriptErrors.push(String(event.message)));
window.addEventListener("unhandledrejection", (event) =>
  window.scriptErrors.push(String(event.reason)),
);

const invoke = window.__TAURI__.core.invoke;
const $ = (id) => document.getElementById(id);

let running = false;
let testTone = null;
let settings = null;
let activePage = "instruments";

// ---- Formatting (units are converted here only; Rust works in SI) ----

const number = (value, digits = 0) =>
  value === null || value === undefined ? "–" : value.toFixed(digits);

// Typographic minus, and never "-0.0".
function signed(value, digits = 1) {
  if (value === null || value === undefined) return "–";
  const rounded = Number(value.toFixed(digits));
  return (rounded > 0 ? "+" : rounded < 0 ? "−" : "") + Math.abs(rounded).toFixed(digits);
}

const COMPASS = ["N", "NO", "O", "SO", "S", "SW", "W", "NW"];
const compass = (degrees) => COMPASS[Math.round(degrees / 45) % 8];

// ---- Pages ----

function showPage(name) {
  activePage = name;
  for (const page of document.querySelectorAll(".page")) {
    page.classList.toggle("active", page.id === "page-" + name);
  }
  for (const tab of document.querySelectorAll("#tabs button")) {
    tab.classList.toggle("active", tab.dataset.page === name);
  }
  if (name === "map") FlightMap.show();
  if (name === "flights") loadFlights();
}

for (const tab of document.querySelectorAll("#tabs button")) {
  tab.addEventListener("click", () => showPage(tab.dataset.page));
}

// ---- Instruments ----

// Full deflection of the bar, in m/s.
const BAR_RANGE = 5;

// Expected longest normal pause per sensor; anything above is highlighted.
const sensorRows = [
  ["Beschleunigung", "accel", 0.5],
  ["Luftdruck", "pressure", 3],
  ["GPS", "gps", 3],
];

function render(status) {
  const climb = status?.climbRateMps ?? null;
  const climbThreshold = settings?.climbThresholdMps ?? 0.2;
  const sinkThreshold = settings?.sinkThresholdMps ?? -2.5;
  // Same thresholds as the tone: green while it beeps, red while the sink tone sounds.
  const state = climb === null ? "" : climb >= climbThreshold ? "up" : climb <= sinkThreshold ? "down" : "";

  $("climb").textContent = signed(climb);
  $("climb").className = state;

  const fill = $("bar-fill");
  const deflection = climb === null ? 0 : Math.min(Math.abs(climb) / BAR_RANGE, 1);
  fill.className = climb !== null && climb < 0 ? "down" : "up";
  fill.style.height = (deflection * 50).toFixed(1) + "%";

  $("altitude").textContent = number(status?.altitudeM);
  const fix = status?.fix;
  $("speed").textContent = fix?.groundSpeedMps == null ? "–" : (fix.groundSpeedMps * 3.6).toFixed(0);
  $("course").textContent = fix?.courseDeg == null ? "–" : fix.courseDeg.toFixed(0) + "°";
  $("course-label").textContent = fix?.courseDeg == null ? "Kurs" : "Kurs " + compass(fix.courseDeg);

  $("map-climb").textContent = signed(climb);
  $("map-altitude").textContent = number(status?.altitudeM);
  if (fix) FlightMap.setPosition(fix.latitudeDeg, fix.longitudeDeg, fix.courseDeg ?? null);

  renderRecording(status?.recording ?? null);
  if (activePage === "settings") renderSensors(status);
}

let recordingFile = null;

function renderRecording(recording) {
  const element = $("recording");
  recordingFile = running ? (recording?.fileName ?? null) : null;
  element.className = recording?.error ? "bad" : "";
  if (!running || !recording) {
    element.textContent = "";
  } else if (recording.error) {
    element.textContent = "Aufzeichnung: Speichern fehlgeschlagen – " + recording.error;
  } else if (recording.fixCount === 0) {
    element.textContent = "Aufzeichnung: warte auf GPS";
  } else {
    element.textContent = `● Aufzeichnung: ${duration(recording.fixCount)}`;
  }
}

// Seconds as "1:05 h" or "12:07 min".
function duration(seconds) {
  const two = (n) => String(n).padStart(2, "0");
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  return h > 0 ? `${h}:${two(m)} h` : `${m}:${two(seconds % 60)} min`;
}

function renderSensors(status) {
  $("sensors").innerHTML = sensorRows
    .map(([label, key, limit]) => {
      const stats = status?.[key];
      if (!stats) return `<tr><td>${label}</td><td colspan="4">–</td></tr>`;
      const stale = stats.ageS == null || stats.ageS > limit;
      const gap = stats.longestGapS > limit;
      return `<tr>
        <td>${label}</td>
        <td>${stats.total}</td>
        <td>${stats.rateHz.toFixed(1)}</td>
        <td class="${stale ? "bad" : ""}">${number(stats.ageS, 1)}</td>
        <td class="${gap ? "bad" : ""}">${stats.longestGapS.toFixed(2)}</td>
      </tr>`;
    })
    .join("");

  if (status) {
    const log = $("log");
    const atBottom = log.scrollTop + log.clientHeight >= log.scrollHeight - 4;
    log.textContent = status.summaries.join("\n");
    if (atBottom) log.scrollTop = log.scrollHeight;
  }
}

// ---- Start / stop ----

function setRunning(value) {
  running = value;
  $("toggle").textContent = running ? "Stopp" : "Start";
  $("toggle").classList.toggle("primary", !running);
  $("tone-climb").disabled = !running;
  $("tone-sink").disabled = !running;
  if (!running) {
    testTone = null;
    showTestTone();
  }
}

async function toggle() {
  $("toggle").disabled = true;
  try {
    if (running) {
      await invoke("stop");
      setRunning(false);
      $("info").textContent = "gestoppt";
      if (activePage === "flights") loadFlights();
    } else {
      const info = await invoke("start");
      FlightMap.clearTrack();
      setRunning(true);
      const s = info.sensors;
      $("info").textContent = s.native
        ? `Barometer: ${s.altimeter ? "ja" : "NEIN"} · Bewegung: ${s.motion ? "ja" : "NEIN"} · ` +
          `Standort: ${s.locationAuthorization}\nAudio-Session: ${s.audioSession} · Ton: ${info.audio}`
        : `Simulierter Flug (kein Gerät) · Ton: ${info.audio}`;
    }
  } catch (error) {
    $("info").textContent = "Fehler: " + error;
  }
  $("toggle").disabled = false;
}

$("toggle").addEventListener("click", toggle);

// ---- Recorded flights ----

const dateFormat = new Intl.DateTimeFormat("de-DE", {
  weekday: "short",
  day: "2-digit",
  month: "2-digit",
  year: "numeric",
  hour: "2-digit",
  minute: "2-digit",
});

function flightTitle(flight) {
  const start = flight.summary?.startUtc;
  // The browser converts the UTC time to local time.
  return start ? dateFormat.format(new Date(start)) : flight.fileName;
}

async function loadFlights() {
  let flights;
  try {
    flights = await invoke("list_flights");
  } catch (error) {
    $("flights-message").textContent = "Fehler: " + error;
    return;
  }
  $("flights-message").textContent = flights.length === 0 ? "Noch keine Flüge aufgezeichnet." : "";
  const list = $("flights");
  list.replaceChildren();
  for (const flight of flights) list.append(flightElement(flight));
}

// Built with DOM calls instead of innerHTML: file names are not ours to trust.
function flightElement(flight) {
  const element = document.createElement("div");
  element.className = "flight";

  const title = document.createElement("div");
  title.className = "flight-title";
  title.textContent = flightTitle(flight);

  const facts = document.createElement("div");
  facts.className = "flight-facts";
  const s = flight.summary;
  const active = flight.fileName === recordingFile;
  facts.textContent = s
    ? `${active ? "läuft · " : ""}${duration(s.durationS)} · max. ${s.maxAltitudeM} m · ` +
      `${(s.trackLengthM / 1000).toFixed(1)} km`
    : "Datei nicht lesbar";

  const buttons = document.createElement("div");
  buttons.className = "row";

  const show = document.createElement("button");
  show.className = "button";
  show.textContent = "Auf Karte";
  show.disabled = !s;
  show.addEventListener("click", () => showFlightOnMap(flight));

  // Deleting cannot be undone, so it takes two taps.
  const remove = document.createElement("button");
  remove.className = "button";
  remove.textContent = "Löschen";
  remove.disabled = active;
  let armed = false;
  remove.addEventListener("click", async () => {
    if (!armed) {
      armed = true;
      remove.textContent = "Wirklich löschen?";
      remove.classList.add("on");
      setTimeout(() => {
        armed = false;
        remove.textContent = "Löschen";
        remove.classList.remove("on");
      }, 3000);
      return;
    }
    try {
      await invoke("delete_flight", { fileName: flight.fileName });
    } catch (error) {
      $("flights-message").textContent = "Löschen fehlgeschlagen: " + error;
    }
    loadFlights();
  });

  buttons.append(show, remove);
  element.append(title, facts, buttons);
  return element;
}

async function showFlightOnMap(flight) {
  try {
    const points = await invoke("flight_track", { fileName: flight.fileName });
    FlightMap.showSavedFlight(points);
    $("saved-flight-label").textContent = flightTitle(flight);
    $("saved-flight").style.display = "flex";
    showPage("map");
  } catch (error) {
    $("flights-message").textContent = "Fehler: " + error;
  }
}

$("close-saved-flight").addEventListener("click", () => {
  FlightMap.hideSavedFlight();
  $("saved-flight").style.display = "none";
});

// ---- Settings ----

// [input id, output id, field of the settings object, how to show the value]
const settingInputs = [
  ["set-climb", "out-climb", "climbThresholdMps", (v) => signed(v) + " m/s"],
  ["set-sink", "out-sink", "sinkThresholdMps", (v) => signed(v) + " m/s"],
  ["set-volume", "out-volume", "volume", (v) => Math.round(v * 100) + " %"],
  ["set-damping", "out-damping", "dampingS", (v) => v.toFixed(1) + " s"],
];

function showSettings() {
  for (const [input, output, field, format] of settingInputs) {
    $(input).value = settings[field];
    $(output).textContent = format(settings[field]);
  }
}

async function applySettings(changed) {
  try {
    await invoke("set_settings", { settings: changed });
    settings = changed;
    $("settings-message").textContent = "";
  } catch (error) {
    // Rust rejected the values (or could not save them): show why.
    $("settings-message").textContent = "Nicht übernommen: " + error;
  }
  showSettings();
}

for (const [input, output, field, format] of settingInputs) {
  // "input" fires while dragging: only update the number. "change" fires on release:
  // apply and save.
  $(input).addEventListener("input", () => {
    $(output).textContent = format(Number($(input).value));
  });
  $(input).addEventListener("change", () => {
    applySettings({ ...settings, [field]: Number($(input).value) });
  });
}

$("reset-settings").addEventListener("click", async () => {
  applySettings(await invoke("default_settings"));
});

function showTestTone() {
  $("tone-climb").classList.toggle("on", testTone === 2.5);
  $("tone-sink").classList.toggle("on", testTone === -4);
}

async function toggleTone(climb) {
  testTone = testTone === climb ? null : climb;
  await invoke("test_tone", { climbRateMps: testTone });
  showTestTone();
}

$("tone-climb").addEventListener("click", () => toggleTone(2.5));
$("tone-sink").addEventListener("click", () => toggleTone(-4));

// ---- Map ----

FlightMap.configure({
  onFollowChange: (follow) => $("follow").classList.toggle("on", follow),
  onMessage: (text) => {
    $("map-message").textContent = text;
    $("map-message").style.display = text ? "block" : "none";
  },
});
$("follow").addEventListener("click", () => FlightMap.toggleFollow());

$("save-map-style").addEventListener("click", async () => {
  try {
    const url = await invoke("set_map_style", { url: $("set-map-style").value });
    $("set-map-style").value = url;
    FlightMap.setStyle(url);
    $("settings-message").textContent = "";
  } catch (error) {
    $("settings-message").textContent = "Nicht übernommen: " + error;
  }
});

// ---- Polling ----
//
// The web view is paused while the screen is locked; that is fine, because everything
// important runs in Rust. After unlocking, polling simply resumes and the track
// request below fetches all points recorded in the meantime.

async function pollStatus() {
  try {
    const status = await invoke("status");
    if (status && !running) setRunning(true);
    render(status);
  } catch (error) {
    $("info").textContent = "Fehler: " + error;
  }
}

async function pollTrack() {
  if (!running) return;
  try {
    const points = await invoke("track", { from: FlightMap.trackLength() });
    if (points.length > 0) FlightMap.addTrackPoints(points);
  } catch (error) {
    console.warn("track", error);
  }
}

async function init() {
  render(null);
  settings = await invoke("get_settings");
  showSettings();
  const style = await invoke("get_map_style");
  $("set-map-style").value = style;
  FlightMap.setStyle(style);
  setInterval(pollStatus, 200);
  setInterval(pollTrack, 1000);
}

init().catch((error) => {
  $("info").textContent = "Fehler beim Start: " + error;
});
