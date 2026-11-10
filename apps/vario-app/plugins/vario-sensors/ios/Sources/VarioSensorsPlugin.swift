// Native sensor bridge for iOS.
//
// Starts GPS, barometer and motion updates, configures the audio session, and hands
// every sample straight to Rust through the C functions exported by the `sensor-bridge`
// crate. Nothing here goes through the web view, so it keeps working while the screen
// is locked (given the background modes "audio" and "location" in Info.plist).
//
// NOTE: compiles and runs in the iOS simulator (GPS only; the simulator has no barometer
// and no motion sensors). Not yet run on a real device.

import AVFoundation
import CoreLocation
import CoreMotion
import SwiftRs
import Tauri
import UIKit
import WebKit

// MARK: - Rust functions (crates/sensor-bridge/src/lib.rs)

@_silgen_name("vario_push_accel")
private func varioPushAccel(_ timeS: Double, _ verticalMps2: Double)

@_silgen_name("vario_push_pressure")
private func varioPushPressure(_ timeS: Double, _ pressurePa: Double)

@_silgen_name("vario_push_gps")
private func varioPushGps(
  _ timeS: Double,
  _ unixTimeS: Double,
  _ latitudeDeg: Double,
  _ longitudeDeg: Double,
  _ altitudeM: Double,
  _ speedMps: Double,
  _ courseDeg: Double,
  _ verticalAccuracyM: Double
)

private let standardGravity = 9.80665
private let motionRateHz = 50.0

// MARK: - Sensors

/// Owns the sensor managers. Must be created and used on the main thread.
final class SensorBridge: NSObject, CLLocationManagerDelegate {
  private let locationManager = CLLocationManager()
  private let altimeter = CMAltimeter()
  private let motionManager = CMMotionManager()

  /// Sensor callbacks run on this queue, one at a time, off the main thread.
  private let sensorQueue: OperationQueue = {
    let queue = OperationQueue()
    queue.name = "vario.sensors"
    queue.maxConcurrentOperationCount = 1
    queue.qualityOfService = .userInteractive
    return queue
  }()

  func start() -> JSObject {
    let audioSession = configureAudioSession()
    startLocation()
    let hasAltimeter = startAltimeter()
    let hasMotion = startMotion()

    return [
      "native": true,
      "altimeter": hasAltimeter,
      "motion": hasMotion,
      "locationAuthorization": authorizationName(locationManager.authorizationStatus),
      "audioSession": audioSession,
    ]
  }

  func stop() {
    locationManager.stopUpdatingLocation()
    altimeter.stopRelativeAltitudeUpdates()
    motionManager.stopDeviceMotionUpdates()
    try? AVAudioSession.sharedInstance().setActive(
      false, options: .notifyOthersOnDeactivation)
  }

  // MARK: Audio session

  /// Category "playback": sound plays with the silent switch on and may continue in
  /// the background. Must be active before Rust opens the audio output.
  private func configureAudioSession() -> String {
    let session = AVAudioSession.sharedInstance()
    do {
      try session.setCategory(.playback, mode: .default, options: [])
      // Short buffers keep the delay between climb rate and tone small.
      try session.setPreferredIOBufferDuration(0.005)
      try session.setActive(true)
      return "ok"
    } catch {
      return "error: \(error.localizedDescription)"
    }
  }

  // MARK: Location

  private func startLocation() {
    locationManager.delegate = self
    locationManager.desiredAccuracy = kCLLocationAccuracyBest
    locationManager.distanceFilter = kCLDistanceFilterNone
    locationManager.activityType = .airborne
    locationManager.pausesLocationUpdatesAutomatically = false
    // Requires "location" in UIBackgroundModes, otherwise this line crashes the app.
    locationManager.allowsBackgroundLocationUpdates = true
    locationManager.showsBackgroundLocationIndicator = true
    locationManager.requestAlwaysAuthorization()
    locationManager.startUpdatingLocation()
  }

  func locationManagerDidChangeAuthorization(_ manager: CLLocationManager) {
    // iOS first grants "when in use"; asking again offers the upgrade to "always".
    if manager.authorizationStatus == .authorizedWhenInUse {
      manager.requestAlwaysAuthorization()
    }
  }

  func locationManager(_ manager: CLLocationManager, didUpdateLocations locations: [CLLocation]) {
    // Motion and barometer timestamps count seconds since boot. Convert the wall
    // clock time of each fix to that same clock so Rust sees one common time base.
    let uptime = ProcessInfo.processInfo.systemUptime
    let now = Date()
    for location in locations where location.horizontalAccuracy >= 0 {
      let timeS = uptime - now.timeIntervalSince(location.timestamp)
      varioPushGps(
        timeS,
        location.timestamp.timeIntervalSince1970,
        location.coordinate.latitude,
        location.coordinate.longitude,
        location.altitude,
        location.speed,
        location.course,
        location.verticalAccuracy
      )
    }
  }

  func locationManager(_ manager: CLLocationManager, didFailWithError error: Error) {
    NSLog("vario: location error: \(error.localizedDescription)")
  }

  // MARK: Barometer

  private func startAltimeter() -> Bool {
    guard CMAltimeter.isRelativeAltitudeAvailable() else { return false }
    altimeter.startRelativeAltitudeUpdates(to: sensorQueue) { data, error in
      guard let data = data else {
        if let error = error {
          NSLog("vario: altimeter error: \(error.localizedDescription)")
        }
        return
      }
      // CoreMotion reports kilopascal.
      varioPushPressure(data.timestamp, data.pressure.doubleValue * 1000.0)
    }
    return true
  }

  // MARK: Acceleration

  private func startMotion() -> Bool {
    guard motionManager.isDeviceMotionAvailable else { return false }
    motionManager.deviceMotionUpdateInterval = 1.0 / motionRateHz
    motionManager.startDeviceMotionUpdates(using: .xArbitraryZVertical, to: sensorQueue) {
      motion, error in
      guard let motion = motion else {
        if let error = error {
          NSLog("vario: motion error: \(error.localizedDescription)")
        }
        return
      }
      // Device motion splits the measured acceleration into `gravity` (unit vector
      // pointing down, device frame) and `userAcceleration` (the rest, in g).
      //
      // Apple's accelerometer reads -1 g on the axis pointing up when the phone lies
      // still, i.e. it reports "gravity minus true acceleration". Therefore
      // userAcceleration = -(true acceleration), and with "up" = -gravity the upward
      // acceleration is the plain dot product of the two vectors.
      //
      // TO VERIFY ON THE DEVICE: lifting the phone quickly must show a positive
      // climb rate first. If it dips first, this sign is wrong.
      let user = motion.userAcceleration
      let gravity = motion.gravity
      let upG = user.x * gravity.x + user.y * gravity.y + user.z * gravity.z
      varioPushAccel(motion.timestamp, upG * standardGravity)
    }
    return true
  }

  private func authorizationName(_ status: CLAuthorizationStatus) -> String {
    switch status {
    case .authorizedAlways: return "always"
    case .authorizedWhenInUse: return "whenInUse"
    case .denied: return "denied"
    case .restricted: return "restricted"
    case .notDetermined: return "notDetermined"
    @unknown default: return "unknown"
    }
  }
}

// MARK: - Tauri plugin

class VarioSensorsPlugin: Plugin {
  private var bridge: SensorBridge?

  @objc public func start(_ invoke: Invoke) {
    // Location and audio session must be set up on the main thread.
    DispatchQueue.main.async {
      if self.bridge == nil {
        self.bridge = SensorBridge()
      }
      invoke.resolve(self.bridge!.start())
    }
  }

  @objc public func stop(_ invoke: Invoke) {
    DispatchQueue.main.async {
      self.bridge?.stop()
      invoke.resolve()
    }
  }
}

@_cdecl("init_plugin_vario_sensors")
func initPlugin() -> Plugin {
  return VarioSensorsPlugin()
}
