// swift-tools-version: 5.10
import Foundation
import PackageDescription

let packageDir = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
let repoRoot = packageDir.deletingLastPathComponent().deletingLastPathComponent()
let libDir = ProcessInfo.processInfo.environment["CLIPSYNC_LIB_DIR"]
    ?? repoRoot.appendingPathComponent("target/release").path

let package = Package(
    name: "ClipSyncMac",
    platforms: [
        .macOS(.v13),
    ],
    products: [
        .executable(name: "ClipSyncMac", targets: ["ClipSyncMac"]),
    ],
    targets: [
        .target(
            name: "CClipSync",
            path: "Sources/CClipSync"
        ),
        .executableTarget(
            name: "ClipSyncMac",
            dependencies: ["CClipSync"],
            path: "Sources/ClipSyncMac",
            linkerSettings: [
                .unsafeFlags(["-L\(libDir)", "-lclipsync_core"]),
                .linkedFramework("AppKit"),
                .linkedFramework("SwiftUI"),
                .linkedFramework("CoreImage"),
                .linkedFramework("Security"),
                .linkedFramework("ServiceManagement"),
                .linkedFramework("SystemConfiguration"),
            ]
        ),
    ]
)
