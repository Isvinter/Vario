// swift-tools-version:5.5

import PackageDescription

let package = Package(
    name: "tauri-plugin-vario-sensors",
    platforms: [
        .macOS(.v10_13),
        .iOS(.v15),
    ],
    products: [
        .library(
            name: "tauri-plugin-vario-sensors",
            type: .static,
            targets: ["tauri-plugin-vario-sensors"])
    ],
    dependencies: [
        // Provided by Tauri's build script next to this package.
        .package(name: "Tauri", path: "../.tauri/tauri-api")
    ],
    targets: [
        .target(
            name: "tauri-plugin-vario-sensors",
            dependencies: [
                .byName(name: "Tauri")
            ],
            path: "Sources")
    ]
)
