//! Offline maps: region packages from the outdoor_app project, read with its
//! `core-maps` crate.
//!
//! A region is a folder with `manifest.json` and PMTiles files (base map, elevation,
//! contour lines, ...), built by the outdoor_app's region builder. Regions are copied
//! into the `maps` folder next to `flights` (on iOS visible in the "Files" app, filled
//! from the Mac with the Finder).
//!
//! The map asks for tiles through the URI scheme `tiles`, answered here in Rust:
//!
//! ```text
//! tiles://localhost/<layer>/<z>/<x>/<y>          tile, or 204 if no region has it
//! tiles://localhost/fonts/<font>/<range>.pbf     glyphs bundled with the app
//! ```
//!
//! (On Windows and Android, Tauri spells the same as `http://tiles.localhost/...`.)

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use core_maps::{Contours, RegionStore, StyleOptions, TilePath};
use serde::Serialize;
use tauri::http::{Request, Response, StatusCode, header};
use tauri::{AppHandle, Manager};

pub const SCHEME: &str = "tiles";

/// Number of tiles served so far; the smoke test reports it.
pub static TILES_SERVED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(any(windows, target_os = "android"))]
const TILES_BASE: &str = "http://tiles.localhost/";
#[cfg(not(any(windows, target_os = "android")))]
const TILES_BASE: &str = "tiles://localhost/";

/// Where the glyph files live among the bundled frontend assets (`ui/map/fonts`).
const FONT_ASSETS: &str = "map/fonts/";

/// The map style for installed regions, with placeholders for tile and font URLs.
/// Taken from the outdoor_app (`styles/topo-summer.json`).
const TOPO_STYLE: &str = include_str!("../map-styles/topo-summer.json");

/// The installed regions. Read again whenever the map style is requested, so regions
/// copied in while the app runs show up when the map is opened.
///
/// `Arc` (a shared pointer with a reference count): a tile request copies the pointer
/// and releases the lock at once, so tiles are read in parallel and a reload does not
/// wait for running requests.
#[derive(Default)]
pub struct Maps {
    store: Mutex<Option<Arc<RegionStore>>>,
}

/// An installed region as the user interface shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionInfo {
    pub id: String,
    pub name: String,
    pub size_bytes: u64,
}

/// What the map page needs to know about the offline maps.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfflineMaps {
    /// The folder to copy regions into.
    pub dir: Option<String>,
    pub regions: Vec<RegionInfo>,
    /// The filled-in map style (JSON), or `None` without installed regions.
    pub style: Option<String>,
}

/// The folder holding the regions.
pub fn dir(app: &AppHandle) -> Option<PathBuf> {
    crate::data_dir(app).map(|dir| dir.join("maps"))
}

/// Reads the regions folder again and returns the regions and the style for them.
pub fn reload(app: &AppHandle) -> Result<OfflineMaps, String> {
    let Some(dir) = dir(app) else {
        return Ok(OfflineMaps {
            dir: None,
            regions: Vec::new(),
            style: None,
        });
    };
    let store = RegionStore::open(&dir).map_err(|error| error.to_string())?;
    let installed: Vec<_> = store
        .installed()
        .into_iter()
        .filter(|region| region.enabled)
        .collect();

    let regions = installed
        .iter()
        .map(|region| RegionInfo {
            id: region.manifest.id.clone(),
            name: region.manifest.name.clone(),
            size_bytes: region.bytes,
        })
        .collect();

    let style = if installed.is_empty() {
        None
    } else {
        // Smallest box around all regions, and every attribution line once.
        let mut bounds = installed[0].manifest.bbox;
        let mut attribution: Vec<String> = Vec::new();
        for region in &installed {
            let [west, south, east, north] = region.manifest.bbox;
            bounds = [
                bounds[0].min(west),
                bounds[1].min(south),
                bounds[2].max(east),
                bounds[3].max(north),
            ];
            for line in &region.manifest.attribution {
                if !attribution.contains(line) {
                    attribution.push(line.clone());
                }
            }
        }
        let options = StyleOptions {
            tiles_base: TILES_BASE,
            bounds: Some(bounds),
            attribution: &attribution,
            contours: Contours::Tiles,
        };
        Some(core_maps::offline_style(TOPO_STYLE, &options).map_err(|error| error.to_string())?)
    };

    *app.state::<Maps>()
        .store
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(store));
    Ok(OfflineMaps {
        dir: Some(dir.to_string_lossy().into_owned()),
        regions,
        style,
    })
}

fn response(status: StatusCode, mime: &str, body: Vec<u8>) -> Response<Vec<u8>> {
    let mut response = Response::new(body);
    *response.status_mut() = status;
    let headers = response.headers_mut();
    // The page lives on another origin (tauri://localhost).
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        header::HeaderValue::from_static("*"),
    );
    if let Ok(value) = header::HeaderValue::from_str(mime) {
        headers.insert(header::CONTENT_TYPE, value);
    }
    response
}

fn empty(status: StatusCode) -> Response<Vec<u8>> {
    response(status, "application/octet-stream", Vec::new())
}

/// Answers one request to the `tiles` scheme. Blocking: reads from disk.
pub fn respond(app: &AppHandle, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let path = request.uri().path();
    if let Some(font) = path.strip_prefix("/fonts/") {
        // Font names contain spaces. A range the app does not bundle is answered with
        // an empty glyph set, so labels render with what is there.
        let asset = format!("{FONT_ASSETS}{}", font.replace("%20", " "));
        let glyphs = app
            .asset_resolver()
            .get(asset)
            .map(|asset| asset.bytes)
            .unwrap_or_default();
        return response(StatusCode::OK, "application/x-protobuf", glyphs);
    }
    let Ok(tile) = path.parse::<TilePath>() else {
        return empty(StatusCode::BAD_REQUEST);
    };
    let store = app
        .state::<Maps>()
        .store
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let Some(store) = store else {
        return empty(StatusCode::NO_CONTENT);
    };
    match store.tile(tile.layer, tile.z, tile.x, tile.y) {
        Ok(Some(tile)) => {
            TILES_SERVED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            response(StatusCode::OK, tile.mime, tile.data)
        }
        Ok(None) => empty(StatusCode::NO_CONTENT),
        Err(error) => {
            eprintln!("map tile {path}: {error}");
            empty(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}
