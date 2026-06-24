import Cocoa
import SwiftUI

@MainActor
class AppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate {
    private var statusItem: NSStatusItem!
    private var window: NSWindow!
    private var serverManager = ServerManager()
    private var store = ConnectionStore()

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)

        setupMenu()
        setupStatusBar()
        setupWindow()
        serverManager.start()
        showWindow()
    }

    // Regular dock app: clicking the dock icon with no visible window reshows it.
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag { showWindow() }
        return true
    }

    func applicationWillTerminate(_ notification: Notification) {
        serverManager.stop()
    }

    private func setupMenu() {
        let mainMenu = NSMenu()

        // App menu
        let appItem = NSMenuItem()
        mainMenu.addItem(appItem)
        let appMenu = NSMenu()
        appMenu.addItem(
            withTitle: "Quit pluk",
            action: #selector(NSApplication.terminate(_:)),
            keyEquivalent: "q"
        )
        appItem.submenu = appMenu

        // Edit menu — without this, the standard text-editing shortcuts
        // (⌘A/⌘C/⌘V/⌘X/⌘Z) have no menu items to dispatch through the responder
        // chain, so they never reach focused text fields. Items target the first
        // responder (nil), so AppKit routes them to whatever field is editing.
        let editItem = NSMenuItem()
        mainMenu.addItem(editItem)
        let editMenu = NSMenu(title: "Edit")
        editMenu.addItem(withTitle: "Undo", action: Selector(("undo:")), keyEquivalent: "z")
        let redo = editMenu.addItem(withTitle: "Redo", action: Selector(("redo:")), keyEquivalent: "z")
        redo.keyEquivalentModifierMask = [.command, .shift]
        editMenu.addItem(.separator())
        editMenu.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        editMenu.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        editMenu.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        editMenu.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        editItem.submenu = editMenu

        NSApp.mainMenu = mainMenu
    }

    private func setupStatusBar() {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        guard let button = statusItem.button else { return }

        button.image = Self.menuBarIcon()
        button.imagePosition = .imageLeft
        button.target = self
        button.action = #selector(toggleWindow)
        button.sendAction(on: [.leftMouseUp, .rightMouseUp])
    }

    /// Menu bar mark — template PDF tinted by the system (black in light bar, white in dark).
    /// Loads from the bundled Resources (release: Bundle.main, dev: Bundle.module),
    /// falling back to an SF Symbol if the resource is missing.
    private static func menuBarIcon() -> NSImage? {
        let url = Bundle.main.url(forResource: "MenuBarIcon", withExtension: "png")
            ?? Bundle.module.url(forResource: "MenuBarIcon", withExtension: "png")
        guard let url, let image = NSImage(contentsOf: url) else {
            return NSImage(systemSymbolName: "cable.connector", accessibilityDescription: "pluk")
        }
        image.size = NSSize(width: 30, height: 18)
        image.isTemplate = true
        return image
    }

    private func setupWindow() {
        window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 700, height: 540),
            styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        window.title = "pluk"
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.isMovableByWindowBackground = true
        window.isReleasedWhenClosed = false
        // Let the window's vibrancy show through, so Liquid Glass surfaces refract.
        window.isOpaque = false
        window.backgroundColor = .clear
        window.delegate = self
        window.setFrameAutosaveName("PlukMainWindow")
        window.center()
        window.contentViewController = NSHostingController(rootView: ContentView(store: store, serverManager: serverManager))
    }

    @objc private func toggleWindow() {
        if window.isVisible {
            window.orderOut(nil)
        } else {
            showWindow()
        }
    }

    private func showWindow() {
        if !window.isVisible { window.center() }
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
    }
}

let app = NSApplication.shared
let delegate = MainActor.assumeIsolated { AppDelegate() }
app.delegate = delegate
app.run()
