import AppKit
import SwiftUI

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var statusItem: NSStatusItem?
    private var popover: NSPopover?
    private var core: CoreBridge?
    private var monitor: PasteboardMonitor?

    private let defaultRelayURL = "wss://clipsync-relay.takvir-tur.workers.dev"

    func applicationDidFinishLaunching(_ notification: Notification) {
        let appSupport = FileManager.default
            .urls(for: .applicationSupportDirectory, in: .userDomainMask)
            .first!
            .appendingPathComponent("ClipSync", isDirectory: true)

        let relayOverride = ProcessInfo.processInfo.environment["CLIPSYNC_RELAY_URL"]
        let relayURL = (relayOverride?.isEmpty == false) ? relayOverride! : defaultRelayURL
        guard let core = CoreBridge(dataDir: appSupport, relayURL: relayURL) else {
            NSApp.terminate(nil)
            return
        }
        self.core = core

        let monitor = PasteboardMonitor(core: core)
        self.monitor = monitor

        let popover = NSPopover()
        popover.behavior = .transient
        popover.contentSize = NSSize(width: 320, height: 420)
        popover.contentViewController = NSHostingController(rootView: PopoverView(monitor: monitor))
        self.popover = popover

        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        if let button = item.button {
            button.image = NSImage(systemSymbolName: "doc.on.clipboard", accessibilityDescription: "ClipSync")
            button.action = #selector(togglePopover(_:))
            button.target = self
        }
        self.statusItem = item

        NSAppleEventManager.shared().setEventHandler(
            self,
            andSelector: #selector(handleGetURLEvent(_:withReplyEvent:)),
            forEventClass: AEEventClass(kInternetEventClass),
            andEventID: AEEventID(kAEGetURL)
        )
    }

    func applicationWillTerminate(_ notification: Notification) {
        monitor?.stop()
        core?.shutdown()
    }

    @objc private func togglePopover(_ sender: Any?) {
        guard let button = statusItem?.button, let popover = popover else { return }
        if popover.isShown {
            popover.performClose(sender)
        } else {
            popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
            NSApp.activate(ignoringOtherApps: true)
        }
    }

    @objc private func handleGetURLEvent(
        _ event: NSAppleEventDescriptor,
        withReplyEvent replyEvent: NSAppleEventDescriptor
    ) {
        if let urlString = event.paramDescriptor(forKeyword: AEKeyword(keyDirectObject))?.stringValue {
            _ = monitor?.joinCluster(uri: urlString)
        }
    }
}
