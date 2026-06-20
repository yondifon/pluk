// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "Pluk",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "Pluk", targets: ["Pluk"])
    ],
    targets: [
        .executableTarget(
            name: "Pluk",
            path: "Sources",
            resources: [
                .copy("Resources/MenuBarIcon.png"),
                .copy("Resources/AdapterLogos"),
            ]
        )
    ]
)
