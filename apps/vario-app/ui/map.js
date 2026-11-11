// The map page: base map, flown track and own position.
//
// The map only draws. Position and track come from Rust (see app.js), so nothing is
// lost while the web view is paused with the screen locked: after unlocking, the
// missing track points are simply fetched.

"use strict";

const FlightMap = (() => {
  // Shown when the configured style cannot be loaded (e.g. no network): a plain
  // background, so that track and position are still visible.
  const EMPTY_STYLE = {
    version: 8,
    sources: {},
    layers: [{ id: "background", type: "background", paint: { "background-color": "#e8e8e8" } }],
  };
  const TRACK_SOURCE = "track";
  const SAVED_SOURCE = "saved-flight";

  let map = null;
  let marker = null;
  let styleUrl = null;
  let usingFallback = false;
  let follow = true;
  let coordinates = []; // [longitude, latitude] pairs of the track
  let savedCoordinates = []; // the same for a recorded flight being viewed
  let position = null; // { longitude, latitude, course }
  let onFollowChange = () => {};
  let onMessage = () => {};

  function markerElement() {
    const element = document.createElement("div");
    element.className = "position-marker";
    // Arrow pointing up; rotated to the course over ground.
    element.innerHTML =
      '<svg viewBox="0 0 26 26" width="26" height="26">' +
      '<path d="M13 1 L23 24 L13 19 L3 24 Z" fill="#000" stroke="#fff" stroke-width="2" stroke-linejoin="round"/>' +
      "</svg>";
    return element;
  }

  // Sources and layers belong to the style, so they must be added again after every
  // style change.
  function addTrackLayers() {
    if (map.getSource(TRACK_SOURCE)) return;
    // The recorded flight being viewed goes below the live track.
    addLine(SAVED_SOURCE, savedCoordinates, "#1a4fd6");
    addLine(TRACK_SOURCE, coordinates, "#c2185b");
  }

  function addLine(source, points, color) {
    map.addSource(source, { type: "geojson", data: lineData(points) });
    // White casing under a dark line: visible on light and dark map areas.
    map.addLayer({
      id: source + "-casing",
      type: "line",
      source,
      layout: { "line-join": "round", "line-cap": "round" },
      paint: { "line-color": "#fff", "line-width": 7 },
    });
    map.addLayer({
      id: source + "-line",
      type: "line",
      source,
      layout: { "line-join": "round", "line-cap": "round" },
      paint: { "line-color": color, "line-width": 4 },
    });
  }

  function lineData(points) {
    return {
      type: "Feature",
      properties: {},
      geometry: { type: "LineString", coordinates: points },
    };
  }

  function refresh(source, points) {
    const mapSource = map && map.getSource(source);
    if (mapSource) mapSource.setData(lineData(points));
  }

  const refreshTrack = () => refresh(TRACK_SOURCE, coordinates);

  function fitSavedFlight() {
    if (!map || savedCoordinates.length === 0) return;
    const bounds = new maplibregl.LngLatBounds(savedCoordinates[0], savedCoordinates[0]);
    for (const point of savedCoordinates) bounds.extend(point);
    map.fitBounds(bounds, { padding: 70, maxZoom: 15, duration: 0 });
  }

  function setFollow(value) {
    follow = value;
    onFollowChange(follow);
    if (follow && position && map) {
      map.easeTo({ center: [position.longitude, position.latitude], duration: 300 });
    }
  }

  function create() {
    map = new maplibregl.Map({
      container: "map",
      style: styleUrl,
      center: position ? [position.longitude, position.latitude] : [11.0, 47.6],
      zoom: position ? 14 : 8,
      // The attribution must always be visible (terms of the tile providers).
      attributionControl: { compact: false },
      dragRotate: false,
      pitchWithRotate: false,
    });
    map.touchZoomRotate.disableRotation();
    map.addControl(new maplibregl.ScaleControl({ unit: "metric" }), "bottom-left");

    map.on("style.load", addTrackLayers);
    // A flight chosen before the map existed: show it once the map has its size.
    map.once("load", fitSavedFlight);
    // Moving the map by hand stops following; the "Folgen" button turns it back on.
    map.on("dragstart", () => setFollow(false));
    map.on("error", (event) => {
      // If the style itself cannot be loaded there is no map at all: fall back once.
      if (!map.isStyleLoaded() && !usingFallback && !map.getSource(TRACK_SOURCE)) {
        usingFallback = true;
        map.setStyle(EMPTY_STYLE);
        onMessage("Karte nicht ladbar (keine Verbindung?). Track und Position werden trotzdem gezeigt.");
      } else {
        console.warn("map error", event.error);
      }
    });

    marker = new maplibregl.Marker({ element: markerElement(), rotationAlignment: "map" });
    if (position) showPosition();
  }

  function showPosition() {
    marker.setLngLat([position.longitude, position.latitude]);
    marker.setRotation(position.course ?? 0);
    marker.addTo(map);
    if (follow) map.jumpTo({ center: [position.longitude, position.latitude] });
  }

  return {
    configure(options) {
      onFollowChange = options.onFollowChange;
      onMessage = options.onMessage;
    },

    /** Sets the style URL; reloads the map if it already exists. */
    setStyle(url) {
      styleUrl = url;
      usingFallback = false;
      onMessage("");
      if (map) map.setStyle(url);
    },

    /** Call when the map page becomes visible: WebGL needs a visible container. */
    show() {
      if (!map) {
        create();
      } else {
        map.resize();
        if (savedCoordinates.length > 0 && !follow) fitSavedFlight();
        else if (follow && position) map.jumpTo({ center: [position.longitude, position.latitude] });
      }
    },

    /** Latest position; `course` in degrees or null. */
    setPosition(latitude, longitude, course) {
      const first = position === null;
      position = { latitude, longitude, course };
      if (!map) return;
      if (first && follow) map.jumpTo({ center: [longitude, latitude], zoom: 14 });
      showPosition();
    },

    /** Appends points ({ latitudeDeg, longitudeDeg }) to the track. */
    addTrackPoints(points) {
      for (const point of points) coordinates.push([point.longitudeDeg, point.latitudeDeg]);
      refreshTrack();
    },

    clearTrack() {
      coordinates = [];
      refreshTrack();
    },

    /** Shows a recorded flight (points as from Rust) and zooms to it. */
    showSavedFlight(points) {
      savedCoordinates = points.map((point) => [point.longitudeDeg, point.latitudeDeg]);
      refresh(SAVED_SOURCE, savedCoordinates);
      // Otherwise the map would jump straight back to the own position.
      setFollow(false);
      fitSavedFlight();
    },

    hideSavedFlight() {
      savedCoordinates = [];
      refresh(SAVED_SOURCE, savedCoordinates);
    },

    trackLength: () => coordinates.length,
    toggleFollow: () => setFollow(!follow),
  };
})();
