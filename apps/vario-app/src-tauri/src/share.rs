//! Regions over the home network: the outdoor_app on the desktop shares its regions
//! (its `core-share` server), and this app downloads them into its `maps` folder.
//!
//! The server offers `catalog.json` (the regions with version and size) and the files
//! of each region. Downloading, resuming, checking the SHA-256 checksums and switching
//! to the new version is done by `core-maps` (`RegionStore::install`); a region shows
//! up on the map only once all its files are complete and verified.
//!
//! Downloads run on their own thread and report through the event `region-download`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use core_maps::{DefaultFetcher, Fetcher, MapsError, Progress, RegionStore};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

/// File in the config folder holding the address of the desktop's share.
const SOURCE_FILE_NAME: &str = "map-source.txt";

/// Name of the event that reports download progress and the end of a download.
pub const DOWNLOAD_EVENT: &str = "region-download";

/// Progress events are sent no more often than this.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

/// What a region of the list is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RegionState {
    /// Offered by the desktop, not on this device.
    Available,
    /// On this device and not newer on the desktop (or the desktop is not reachable).
    Installed,
    /// On this device, and the desktop has a newer version.
    UpdateAvailable,
    /// Being downloaded right now.
    Downloading,
}

/// A region on this device or on offer.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionEntry {
    pub id: String,
    pub name: String,
    /// Size of the installed region, or of the offered one if not installed.
    pub size_bytes: u64,
    /// The newest known version: the desktop's if it offers one.
    pub version: String,
    pub installed_version: Option<String>,
    pub state: RegionState,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionList {
    /// The address of the share; empty if none is set.
    pub source: String,
    /// Why the desktop's catalog could not be read; installed regions are listed anyway.
    pub catalog_error: Option<String>,
    /// Ordered by id.
    pub regions: Vec<RegionEntry>,
}

/// Payload of [`DOWNLOAD_EVENT`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadEvent {
    pub region: String,
    /// Bytes on disk so far, and of the complete region.
    pub done: u64,
    pub total: u64,
    /// `running`, `done`, `failed` or `cancelled`.
    pub state: &'static str,
    /// For `failed`: a stable code (`region_download`, `region_checksum`, ... from
    /// `core-maps`) and the message.
    pub error_code: Option<&'static str>,
    pub error: Option<String>,
}

/// The downloads running now, each with the flag that cancels it.
///
/// `Arc<AtomicBool>`: the flag is shared between the download thread, which checks it
/// after every chunk, and the command that sets it. An atomic needs no lock.
#[derive(Default)]
pub struct Downloads {
    running: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl Downloads {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<AtomicBool>>> {
        self.running.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn ids(&self) -> Vec<String> {
        self.lock().keys().cloned().collect()
    }

    /// Registers a download; `None` if this region is already being downloaded.
    fn begin(&self, id: &str) -> Option<Arc<AtomicBool>> {
        let mut running = self.lock();
        if running.contains_key(id) {
            return None;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        running.insert(id.to_owned(), Arc::clone(&cancel));
        Some(cancel)
    }

    fn end(&self, id: &str) {
        self.lock().remove(id);
    }

    fn cancel(&self, id: &str) {
        if let Some(cancel) = self.lock().get(id) {
            cancel.store(true, Ordering::SeqCst);
        }
    }

    fn is_running(&self, id: &str) -> bool {
        self.lock().contains_key(id)
    }
}

/// Puts a typed address into shape: `host:port` becomes `http://host:port/`; an
/// address ending in `.json` is taken as it is. Blank means no source.
pub fn normalise_source(source: &str) -> String {
    let source = source.trim();
    if source.is_empty() {
        return String::new();
    }
    let mut out = if source.contains("://") {
        source.to_owned()
    } else {
        format!("http://{source}")
    };
    if !out.ends_with('/') && !out.ends_with(".json") {
        out.push('/');
    }
    out
}

/// Checks a normalised address: only `http(s)://` with a host and no spaces.
fn check_source(source: &str) -> Result<(), String> {
    if source.is_empty() {
        return Ok(());
    }
    let rest = source
        .strip_prefix("http://")
        .or_else(|| source.strip_prefix("https://"))
        .ok_or("the address must start with http:// or https://")?;
    if rest.starts_with('/') || rest.contains(char::is_whitespace) {
        return Err("not a valid address".to_owned());
    }
    Ok(())
}

/// Installed regions merged with what the catalog at `source` offers.
pub fn list(
    store: &RegionStore,
    source: &str,
    fetcher: &dyn Fetcher,
    downloading: &[String],
) -> RegionList {
    let mut regions: Vec<RegionEntry> = store
        .installed()
        .into_iter()
        .map(|region| RegionEntry {
            id: region.manifest.id.clone(),
            name: region.manifest.name,
            size_bytes: region.bytes,
            version: region.manifest.version.clone(),
            installed_version: Some(region.manifest.version),
            state: RegionState::Installed,
        })
        .collect();
    let mut catalog_error = None;
    if !source.is_empty() {
        match store.catalog(source, fetcher) {
            Ok(catalog) => {
                for offered in catalog.regions {
                    match regions.iter_mut().find(|region| region.id == offered.id) {
                        // Versions are build times like `202611181120`; newer is greater.
                        Some(region) => {
                            if offered.version > region.version {
                                region.version = offered.version;
                                region.state = RegionState::UpdateAvailable;
                            }
                        }
                        None => regions.push(RegionEntry {
                            id: offered.id,
                            name: offered.name,
                            size_bytes: offered.size,
                            version: offered.version,
                            installed_version: None,
                            state: RegionState::Available,
                        }),
                    }
                }
            }
            Err(error) => catalog_error = Some(error.to_string()),
        }
    }
    for region in &mut regions {
        if downloading.contains(&region.id) {
            region.state = RegionState::Downloading;
        }
    }
    regions.sort_by(|a, b| a.id.cmp(&b.id));
    RegionList {
        source: source.to_owned(),
        catalog_error,
        regions,
    }
}

/// Downloads region `id` from `source` into the store in `dir`. Blocks until done.
pub fn install(
    dir: &Path,
    source: &str,
    id: &str,
    fetcher: &dyn Fetcher,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<(), MapsError> {
    if source.is_empty() {
        return Err(MapsError::UnknownRegion(id.to_owned()));
    }
    let store = RegionStore::open(dir)?;
    store
        .install(source, id, fetcher, progress, cancel)
        .map(|_| ())
}

/// The saved address of the share; empty if none.
pub fn load_source(app: &AppHandle) -> String {
    crate::settings::config_file(app, SOURCE_FILE_NAME)
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| text.trim().to_owned())
        .unwrap_or_default()
}

/// Saves the address of the share and returns it as stored.
pub fn save_source(app: &AppHandle, source: &str) -> Result<String, String> {
    let source = normalise_source(source);
    check_source(&source)?;
    crate::settings::write_file(
        crate::settings::config_file(app, SOURCE_FILE_NAME)?,
        &source,
    )?;
    Ok(source)
}

fn maps_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    crate::maps::dir(app).ok_or_else(|| "no maps folder".to_owned())
}

/// The region list. With `catalog`, the desktop's catalog is read too, which blocks
/// (up to the connect timeout of 20 s if the desktop is not reachable); without it the
/// list holds only the installed regions and comes at once.
pub fn regions(app: &AppHandle, catalog: bool) -> Result<RegionList, String> {
    let store = RegionStore::open(&maps_dir(app)?).map_err(|error| error.to_string())?;
    let downloading = app.state::<Downloads>().ids();
    let source = load_source(app);
    let mut list = list(
        &store,
        if catalog { &source } else { "" },
        &DefaultFetcher::new(),
        &downloading,
    );
    list.source = source;
    Ok(list)
}

/// Starts downloading region `id` on its own thread and returns at once. Progress and
/// the end go out as [`DOWNLOAD_EVENT`].
pub fn start_download(app: &AppHandle, id: String) -> Result<(), String> {
    let dir = maps_dir(app)?;
    let source = load_source(app);
    if source.is_empty() {
        return Err("no desktop address set".to_owned());
    }
    let cancel = app
        .state::<Downloads>()
        .begin(&id)
        .ok_or_else(|| format!("region {id} is already being downloaded"))?;
    let thread_app = app.clone();
    let thread_id = id.clone();
    let spawned = std::thread::Builder::new()
        .name(format!("download-{id}"))
        .spawn(move || download(&thread_app, &dir, &source, &thread_id, &cancel));
    if let Err(error) = spawned {
        // Without the thread nothing would ever end the download.
        app.state::<Downloads>().end(&id);
        return Err(error.to_string());
    }
    Ok(())
}

fn download(app: &AppHandle, dir: &Path, source: &str, id: &str, cancel: &AtomicBool) {
    let emit = |event: DownloadEvent| {
        let _ = app.emit(DOWNLOAD_EVENT, event);
    };
    let mut last = Progress {
        region: id.to_owned(),
        done: 0,
        total: 0,
    };
    let mut last_sent: Option<Instant> = None;
    let mut report = |progress: Progress| {
        if last_sent.is_none_or(|at| at.elapsed() >= PROGRESS_INTERVAL) {
            last_sent = Some(Instant::now());
            emit(DownloadEvent {
                region: progress.region.clone(),
                done: progress.done,
                total: progress.total,
                state: "running",
                error_code: None,
                error: None,
            });
        }
        last = progress;
    };
    let result = install(dir, source, id, &DefaultFetcher::new(), &mut report, cancel);
    // Before the last event: the list the page reads afterwards no longer shows
    // the region as downloading.
    app.state::<Downloads>().end(id);
    let (state, error_code, error) = match result {
        Ok(()) => ("done", None, None),
        Err(MapsError::Cancelled) => ("cancelled", None, None),
        Err(error) => {
            eprintln!("download of region {id}: {error}");
            ("failed", Some(error.code()), Some(error.to_string()))
        }
    };
    emit(DownloadEvent {
        region: id.to_owned(),
        done: last.done,
        total: last.total,
        state,
        error_code,
        error,
    });
}

/// Stops the download of a region; the part downloaded so far is kept and continued
/// by the next download.
pub fn cancel_download(app: &AppHandle, id: &str) {
    app.state::<Downloads>().cancel(id);
}

/// Removes an installed region from this device.
pub fn delete(app: &AppHandle, id: &str) -> Result<(), String> {
    if app.state::<Downloads>().is_running(id) {
        return Err(format!("region {id} is being downloaded"));
    }
    let store = RegionStore::open(&maps_dir(app)?).map_err(|error| error.to_string())?;
    store.delete(id).map_err(|error| error.to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use core_maps::{ArchiveInfo, CatalogEntry, Layer, RegionFile, RegionManifest, TileType};
    use sha2::{Digest, Sha256};
    use std::path::PathBuf;

    const BBOX: [f64; 4] = [11.35, 47.35, 11.95, 47.8];

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vario-share-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes a region package with one small base map archive into `dir/<id>`.
    fn write_package(dir: &Path, id: &str, version: &str) -> RegionManifest {
        let package = dir.join(id);
        std::fs::create_dir_all(&package).unwrap();
        let name = Layer::Basemap.file_name();
        let path = package.join(&name);
        let info = ArchiveInfo {
            tile_type: TileType::Mvt,
            bounds: BBOX,
        };
        // Different content per version, so the checksums differ.
        core_maps::write_archive(&path, vec![(7, 68, 44, version.as_bytes().to_vec())], &info)
            .unwrap();
        let data = std::fs::read(&path).unwrap();
        let sha256 = Sha256::digest(&data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let manifest = RegionManifest {
            format: core_maps::FORMAT,
            id: id.to_owned(),
            name: format!("Region {id}"),
            bbox: BBOX,
            version: version.to_owned(),
            attribution: vec!["test".to_owned()],
            routing: None,
            files: vec![RegionFile {
                name,
                size: data.len() as u64,
                sha256,
            }],
            size: data.len() as u64,
        };
        std::fs::write(
            package.join("manifest.json"),
            serde_json::to_string(&manifest).unwrap(),
        )
        .unwrap();
        manifest
    }

    fn write_catalog(dir: &Path, manifests: &[&RegionManifest]) {
        let catalog = core_maps::Catalog {
            format: core_maps::FORMAT,
            regions: manifests
                .iter()
                .map(|manifest| CatalogEntry {
                    id: manifest.id.clone(),
                    name: manifest.name.clone(),
                    bbox: manifest.bbox,
                    version: manifest.version.clone(),
                    size: manifest.size,
                    path: manifest.id.clone(),
                })
                .collect(),
        };
        std::fs::write(
            dir.join("catalog.json"),
            serde_json::to_string(&catalog).unwrap(),
        )
        .unwrap();
    }

    /// A desktop offering `a` (version 2) and `b`, a phone with `a` (version 1).
    fn setup(name: &str) -> (PathBuf, PathBuf) {
        let desktop = temp_dir(&format!("{name}-desktop"));
        let a = write_package(&desktop, "a", "202611180000");
        let b = write_package(&desktop, "b", "202611180000");
        write_catalog(&desktop, &[&a, &b]);
        let phone = temp_dir(&format!("{name}-phone"));
        write_package(&phone, "a", "202611010000");
        (desktop, phone)
    }

    fn source(desktop: &Path) -> String {
        format!("{}/", desktop.display())
    }

    fn states(list: &RegionList) -> Vec<(&str, RegionState)> {
        list.regions
            .iter()
            .map(|region| (region.id.as_str(), region.state))
            .collect()
    }

    #[test]
    fn typed_addresses_are_normalised() {
        assert_eq!(
            normalise_source(" 192.168.1.20:8765 "),
            "http://192.168.1.20:8765/"
        );
        assert_eq!(normalise_source("http://desk:8765"), "http://desk:8765/");
        assert_eq!(
            normalise_source("http://desk/catalog.json"),
            "http://desk/catalog.json"
        );
        assert_eq!(normalise_source("   "), "");
        assert!(check_source(&normalise_source("192.168.1.20:8765")).is_ok());
        assert!(check_source(&normalise_source("ftp://desk")).is_err());
        assert!(check_source(&normalise_source("my desk")).is_err());
    }

    #[test]
    fn list_merges_installed_and_offered_regions() {
        let (desktop, phone) = setup("list");
        let store = RegionStore::open(&phone).unwrap();
        let list = list(&store, &source(&desktop), &DefaultFetcher::new(), &[]);
        assert_eq!(list.catalog_error, None);
        assert_eq!(
            states(&list),
            [
                ("a", RegionState::UpdateAvailable),
                ("b", RegionState::Available)
            ]
        );
        assert_eq!(
            list.regions[0].installed_version.as_deref(),
            Some("202611010000")
        );
        assert_eq!(list.regions[0].version, "202611180000");
        assert_eq!(list.regions[1].installed_version, None);
    }

    #[test]
    fn list_without_desktop_shows_installed_regions() {
        let (desktop, phone) = setup("unreachable");
        let store = RegionStore::open(&phone).unwrap();
        let none = list(&store, "", &DefaultFetcher::new(), &[]);
        assert_eq!(none.catalog_error, None);
        assert_eq!(states(&none), [("a", RegionState::Installed)]);

        let missing = format!("{}/", desktop.join("missing").display());
        let failed = list(&store, &missing, &DefaultFetcher::new(), &["a".to_owned()]);
        assert!(failed.catalog_error.is_some());
        assert_eq!(states(&failed), [("a", RegionState::Downloading)]);
    }

    #[test]
    fn install_downloads_and_updates_regions() {
        let (desktop, phone) = setup("install");
        let cancel = AtomicBool::new(false);
        let mut reports = Vec::new();
        let mut progress = |p: Progress| reports.push((p.done, p.total));
        for id in ["a", "b"] {
            install(
                &phone,
                &source(&desktop),
                id,
                &DefaultFetcher::new(),
                &mut progress,
                &cancel,
            )
            .unwrap();
        }
        assert!(reports.last().is_some_and(|(done, total)| done == total));

        let store = RegionStore::open(&phone).unwrap();
        let list = list(&store, &source(&desktop), &DefaultFetcher::new(), &[]);
        assert_eq!(
            states(&list),
            [("a", RegionState::Installed), ("b", RegionState::Installed)]
        );
        assert_eq!(
            list.regions[0].installed_version.as_deref(),
            Some("202611180000")
        );
    }

    #[test]
    fn cancelled_install_changes_nothing() {
        let (desktop, phone) = setup("cancel");
        let cancel = AtomicBool::new(true);
        let result = install(
            &phone,
            &source(&desktop),
            "b",
            &DefaultFetcher::new(),
            &mut |_| {},
            &cancel,
        );
        assert!(matches!(result, Err(MapsError::Cancelled)));
        let store = RegionStore::open(&phone).unwrap();
        assert_eq!(store.installed().len(), 1);
    }

    #[test]
    fn install_rejects_a_damaged_file() {
        let (desktop, phone) = setup("damaged");
        let file = desktop.join("b").join(Layer::Basemap.file_name());
        let mut data = std::fs::read(&file).unwrap();
        let last = data.len() - 1;
        data[last] ^= 0xff;
        std::fs::write(&file, data).unwrap();

        let cancel = AtomicBool::new(false);
        let result = install(
            &phone,
            &source(&desktop),
            "b",
            &DefaultFetcher::new(),
            &mut |_| {},
            &cancel,
        );
        assert!(
            matches!(result, Err(MapsError::Checksum { .. })),
            "{result:?}"
        );
        let store = RegionStore::open(&phone).unwrap();
        assert!(
            store
                .installed()
                .iter()
                .all(|region| region.manifest.id != "b")
        );
    }

    #[test]
    fn a_region_downloads_only_once_at_a_time() {
        let downloads = Downloads::default();
        let cancel = downloads.begin("a").unwrap();
        assert!(downloads.begin("a").is_none());
        assert!(downloads.is_running("a"));
        downloads.cancel("a");
        assert!(cancel.load(Ordering::SeqCst));
        downloads.end("a");
        assert!(downloads.begin("a").is_some());
    }
}
