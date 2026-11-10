//! Tauri plugin that starts and stops the native sensors.
//!
//! On iOS the work is done by the Swift code in `ios/Sources`: it configures the audio
//! session and starts GPS, barometer and motion updates. The Swift code delivers the
//! samples by calling the C functions of the `sensor-bridge` crate directly; this plugin
//! only carries the "start" and "stop" requests.
//!
//! On all other platforms the plugin does nothing, so the app code is the same
//! everywhere.

use serde::{Deserialize, Serialize};
use tauri::plugin::{Builder, TauriPlugin};
use tauri::{Manager, Runtime};

/// What the native side reports after starting the sensors.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SensorStart {
    /// `true` if native sensors exist on this platform at all.
    pub native: bool,
    /// Barometer available.
    pub altimeter: bool,
    /// Motion data (acceleration and attitude) available.
    pub motion: bool,
    /// Location permission, e.g. "always", "whenInUse", "denied", "notDetermined".
    pub location_authorization: String,
    /// "ok" or an error description.
    pub audio_session: String,
}

// Declares the Swift function `init_plugin_vario_sensors` (see the bottom of
// `ios/Sources/VarioSensorsPlugin.swift`) so that Rust can call it.
#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_vario_sensors);

/// Access to the native sensors. Get it with [`VarioSensorsExt::vario_sensors`].
pub struct VarioSensors<R: Runtime> {
    #[cfg(target_os = "ios")]
    handle: tauri::plugin::PluginHandle<R>,
    // On other platforms nothing needs to be stored, but the type parameter `R` must
    // still be used somewhere; `PhantomData` does that without taking up space.
    #[cfg(not(target_os = "ios"))]
    _runtime: std::marker::PhantomData<fn() -> R>,
}

impl<R: Runtime> VarioSensors<R> {
    /// Configures the audio session and starts all sensors.
    ///
    /// Must not be called on the main thread: the Swift side does its work on the main
    /// thread and this call waits for it.
    pub fn start(&self) -> Result<SensorStart, String> {
        #[cfg(target_os = "ios")]
        {
            self.handle
                .run_mobile_plugin("start", ())
                .map_err(|error| error.to_string())
        }
        #[cfg(not(target_os = "ios"))]
        {
            Ok(SensorStart::default())
        }
    }

    /// Stops all sensors. Same threading rule as [`VarioSensors::start`].
    pub fn stop(&self) -> Result<(), String> {
        #[cfg(target_os = "ios")]
        {
            self.handle
                .run_mobile_plugin::<serde::de::IgnoredAny>("stop", ())
                .map(|_| ())
                .map_err(|error| error.to_string())
        }
        #[cfg(not(target_os = "ios"))]
        {
            Ok(())
        }
    }
}

/// Adds `vario_sensors()` to Tauri's app types.
pub trait VarioSensorsExt<R: Runtime> {
    fn vario_sensors(&self) -> &VarioSensors<R>;
}

impl<R: Runtime, T: Manager<R>> VarioSensorsExt<R> for T {
    fn vario_sensors(&self) -> &VarioSensors<R> {
        self.state::<VarioSensors<R>>().inner()
    }
}

/// Creates the plugin; register it with `tauri::Builder::plugin`.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("vario-sensors")
        .setup(|app, _api| {
            #[cfg(target_os = "ios")]
            let sensors = VarioSensors {
                handle: _api.register_ios_plugin(init_plugin_vario_sensors)?,
            };
            #[cfg(not(target_os = "ios"))]
            let sensors = VarioSensors::<R> {
                _runtime: std::marker::PhantomData,
            };
            app.manage(sensors);
            Ok(())
        })
        .build()
}
