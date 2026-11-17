// Native sensor bridge for iOS.
//
// Starts GPS, barometer and motion updates, configures the audio session, and hands
// every sample straight to Rust through the C functions exported by the `sensor-bridge`
// crate. Nothing here goes through the web view, so it keeps working while the screen
// is locked (given the background modes "audio" and "location" in Info.plist).
//
// NOTE: tested on an iPhone 13 Pro Max (iOS 27.2): motion, barometer and GPS deliver
// without gaps with the screen locked, also outdoors and during a phone call. Resuming
// the tone after an audio interruption is not yet tested on the device.

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

@_silgen_name("vario_audio_session_resumed")
private func varioAudioSessionResumed()

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
/// While an audio interruption is not known to be over, try this often to get the audio
/// session back.
private let audioRetryIntervalS = 2.0

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

  /// Tokens of the notification observers, to remove them again in `stop`.
  private var observers: [NSObjectProtocol] = []
  /// `true` from the start of an audio interruption until the session is active again.
  private var audioInterrupted = false
  private var audioRetryTimer: Timer?

  func start() -> JSObject {
    let audioSession = configureAudioSession()
    observeAudioInterruptions()
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
    for observer in observers {
      NotificationCenter.default.removeObserver(observer)
    }
    observers = []
    audioRetryTimer?.invalidate()
    audioRetryTimer = nil
    audioInterrupted = false
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

  // MARK: Audio interruptions

  /// A phone call, an alarm or Siri interrupts the audio session; afterwards the app has
  /// to activate it again and Rust has to reopen its audio output, otherwise the tone
  /// stays silent. iOS does not always send the "ended" notification, so while an
  /// interruption is open, the session is also retried on a timer and whenever the app
  /// comes to the foreground. The tone resumes even if iOS does not suggest it
  /// (`shouldResume`): a vario must not stay silent after a call.
  private func observeAudioInterruptions() {
    let center = NotificationCenter.default
    let session = AVAudioSession.sharedInstance()
    observers.append(
      center.addObserver(
        forName: AVAudioSession.interruptionNotification, object: session, queue: .main
      ) { [weak self] notification in
        guard let self = self,
          let raw = notification.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
          let type = AVAudioSession.InterruptionType(rawValue: raw)
        else { return }
        switch type {
        case .began:
          NSLog("vario: audio interruption began")
          self.audioInterrupted = true
          self.startAudioRetryTimer()
        case .ended:
          NSLog("vario: audio interruption ended")
          self.resumeAudio()
        @unknown default:
          break
        }
      })
    // The media services restarted (rare): every audio object is invalid now.
    observers.append(
      center.addObserver(
        forName: AVAudioSession.mediaServicesWereResetNotification, object: session,
        queue: .main
      ) { [weak self] _ in
        NSLog("vario: media services were reset")
        self?.audioInterrupted = true
        self?.resumeAudio()
      })
    observers.append(
      center.addObserver(
        forName: UIApplication.didBecomeActiveNotification, object: nil, queue: .main
      ) { [weak self] _ in
        if self?.audioInterrupted == true {
          self?.resumeAudio()
        }
      })
  }

  private func startAudioRetryTimer() {
    audioRetryTimer?.invalidate()
    audioRetryTimer = Timer.scheduledTimer(withTimeInterval: audioRetryIntervalS, repeats: true)
    { [weak self] _ in
      self?.resumeAudio()
    }
  }

  /// Activates the audio session again and tells Rust to reopen its output. Does
  /// nothing while the interruption is still going on (e.g. during the call), then the
  /// timer tries again.
  private func resumeAudio() {
    let result = configureAudioSession()
    guard result == "ok" else {
      NSLog("vario: audio session not available yet: \(result)")
      if audioRetryTimer == nil {
        startAudioRetryTimer()
      }
      return
    }
    audioInterrupted = false
    audioRetryTimer?.invalidate()
    audioRetryTimer = nil
    varioAudioSessionResumed()
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
      // Verified on the device: lifting the phone quickly shows a positive climb
      // rate first.
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
