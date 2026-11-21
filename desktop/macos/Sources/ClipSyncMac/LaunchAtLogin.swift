import Foundation
import ServiceManagement

enum LaunchAtLogin {
    private static let initKey = "clipsync_autostart_initialized"

    static var isEnabled: Bool {
        SMAppService.mainApp.status == .enabled
    }

    @discardableResult
    static func ensureDefaultEnabled() -> Bool {
        let defaults = UserDefaults.standard
        if !defaults.bool(forKey: initKey) {
            defaults.set(true, forKey: initKey)
            try? SMAppService.mainApp.register()
        }
        return isEnabled
    }

    @discardableResult
    static func setEnabled(_ enabled: Bool) -> Bool {
        do {
            if enabled {
                try SMAppService.mainApp.register()
            } else {
                try SMAppService.mainApp.unregister()
            }
        } catch {
            // Registration requires running from a signed/valid app bundle; caller reflects actual status.
        }
        return isEnabled
    }
}
