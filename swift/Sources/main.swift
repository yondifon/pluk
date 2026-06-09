import Cocoa
import SwiftUI

@MainActor
class AppDelegate: NSObject, NSApplicationDelegate {
    private var statusItem: NSStatusItem!
    private var popover: NSPopover!
    private var serverManager = ServerManager()
    private var store = ConnectionStore()

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)

        setupStatusBar()
        setupPopover()
        serverManager.start()
    }

    func applicationWillTerminate(_ notification: Notification) {
        serverManager.stop()
    }

    private func setupStatusBar() {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        guard let button = statusItem.button else { return }

        button.image = NSImage(systemSymbolName: "cable.connector", accessibilityDescription: "pluk")
        button.imagePosition = .imageLeft
        button.target = self
        button.action = #selector(togglePopover)
        button.sendAction(on: [.leftMouseUp, .rightMouseUp])
    }

    private func setupPopover() {
        popover = NSPopover()
        popover.contentSize = NSSize(width: 700, height: 540)
        popover.behavior = .transient
        popover.animates = true

        let contentView = ContentView(store: store)
        popover.contentViewController = NSHostingController(rootView: contentView)
    }

    @objc private func togglePopover() {
        guard let button = statusItem.button else { return }

        if popover.isShown {
            popover.performClose(nil)
        } else {
            NSApp.activate(ignoringOtherApps: true)
            popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
        }
    }
}

let app = NSApplication.shared
let delegate = MainActor.assumeIsolated { AppDelegate() }
app.delegate = delegate
app.run()
