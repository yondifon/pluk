import Cocoa
import SwiftUI

@MainActor
class AppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate {
    private var statusItem: NSStatusItem!
    private var window: NSWindow!
    private var serverManager = ServerManager()
    private var store = ConnectionStore()
    private var updateChecker = UpdateChecker()

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Menu-bar app: no dock icon until the window is explicitly opened
        // from the status item (LSUIElement in Info.plist keeps launch quiet;
        // this covers dev runs, which have no Info.plist).
        NSApp.setActivationPolicy(.accessory)

        setupMenu()
        setupStatusBar()
        setupWindow()
        serverManager.start()
        updateChecker.startPeriodicChecks()
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
        if UpdateChecker.isConfigured {
            let update = appMenu.addItem(
                withTitle: "Check for Updates…",
                action: #selector(checkForUpdates),
                keyEquivalent: ""
            )
            update.target = self
            appMenu.addItem(.separator())
        }
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

        let windowItem = NSMenuItem()
        mainMenu.addItem(windowItem)
        let windowMenu = NSMenu(title: "Window")
        windowMenu.addItem(withTitle: "Minimize", action: #selector(NSWindow.miniaturize(_:)), keyEquivalent: "m")
        windowMenu.addItem(withTitle: "Zoom", action: #selector(NSWindow.performZoom(_:)), keyEquivalent: "")
        windowMenu.addItem(.separator())
        windowMenu.addItem(withTitle: "Bring All to Front", action: #selector(NSApplication.arrangeInFront(_:)), keyEquivalent: "")
        windowItem.submenu = windowMenu
        NSApp.windowsMenu = windowMenu

        NSApp.mainMenu = mainMenu
    }

    private func setupStatusBar() {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        guard let button = statusItem.button else { return }

        // If the user ever Cmd-dragged the icon off the menu bar, macOS
        // persists that removal and the item silently never comes back.
        // Force it visible on every launch — the icon is the only way in.
        statusItem.isVisible = true
        button.image = Self.menuBarIcon()
        button.imagePosition = .imageLeft
        button.target = self
        button.action = #selector(statusItemClicked)
        button.sendAction(on: [.leftMouseUp, .rightMouseUp])
    }

    /// Left click toggles the window; right click shows a menu. As an
    /// accessory app the main menu is hidden while the window is closed,
    /// so this menu is the only always-reachable Quit / update entry point.
    @objc private func statusItemClicked() {
        guard NSApp.currentEvent?.type == .rightMouseUp else {
            toggleWindow()
            return
        }
        let menu = NSMenu()
        menu.addItem(withTitle: window.isVisible ? "Hide pluk" : "Open pluk",
                     action: #selector(toggleWindow), keyEquivalent: "").target = self
        if UpdateChecker.isConfigured {
            menu.addItem(withTitle: "Check for Updates…",
                         action: #selector(checkForUpdates), keyEquivalent: "").target = self
        }
        menu.addItem(.separator())
        menu.addItem(withTitle: "Quit pluk",
                     action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        // Attach transiently: a persistent statusItem.menu would hijack
        // left clicks and break the toggle behavior.
        statusItem.menu = menu
        statusItem.button?.performClick(nil)
        statusItem.menu = nil
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
        window.isOpaque = true
        window.backgroundColor = .textBackgroundColor
        window.delegate = self
        // Don't let the window shrink below the toolbar's intrinsic width, or its
        // trailing actions clip off-screen with no way to scroll to them.
        window.contentMinSize = NSSize(width: 720, height: 520)
        window.contentViewController = NSHostingController(
            rootView: ContentView(store: store, serverManager: serverManager, updateChecker: updateChecker)
        )
        // Restore the user's last size/position; center only on first-ever launch.
        window.setFrameAutosaveName("PlukMainWindow")
        if !window.setFrameUsingName("PlukMainWindow") {
            window.center()
        }
    }

    @objc private func checkForUpdates() {
        showWindow()
        Task { await updateChecker.check() }
    }

    @objc private func toggleWindow() {
        if window.isVisible {
            hideWindow()
        } else {
            showWindow()
        }
    }

    /// Opening the window promotes to a regular app (dock icon, main menu);
    /// closing/hiding it demotes back to a menu-bar-only accessory.
    private func showWindow() {
        NSApp.setActivationPolicy(.regular)
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
    }

    private func hideWindow() {
        window.orderOut(nil)
        NSApp.setActivationPolicy(.accessory)
    }

    // Red close button hides rather than quits (isReleasedWhenClosed = false),
    // so drop the dock icon then too.
    func windowWillClose(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
    }
}

let app = NSApplication.shared
let delegate = MainActor.assumeIsolated { AppDelegate() }
app.delegate = delegate
app.run()
