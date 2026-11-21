import CClipSync
import Foundation

enum ClipSyncKind: UInt8 {
    case text = 1
    case image = 2
}

struct RemoteClipItem {
    let kind: ClipSyncKind
    let data: Data
    let mime: String
    let timestampMs: UInt64
}

struct PinnedClipItem: Identifiable {
    let id: Data
    let kind: ClipSyncKind
    let data: Data
    let mime: String
    let createdAtMs: UInt64
    let updatedAtMs: UInt64

    var idKey: String {
        id.map { String(format: "%02x", $0) }.joined()
    }
}

final class CoreBridge {
    private var engine: OpaquePointer?

    init?(dataDir: URL, relayURL: String? = nil) {
        let dirPath = dataDir.path
        let ptr: OpaquePointer? = dirPath.withCString { dirCStr in
            if let relay = relayURL, !relay.isEmpty {
                return relay.withCString { relayCStr in
                    clipsync_init(dirCStr, relayCStr)
                }
            } else {
                return clipsync_init(dirCStr, nil)
            }
        }
        guard let valid = ptr else { return nil }
        self.engine = valid
    }

    deinit {
        shutdown()
    }

    func shutdown() {
        if let eng = engine {
            clipsync_shutdown(eng)
            engine = nil
        }
    }

    @discardableResult
    func pushLocal(kind: ClipSyncKind, data: Data) -> Bool {
        guard let eng = engine, !data.isEmpty else { return false }
        return data.withUnsafeBytes { raw in
            guard let ptr = raw.baseAddress?.assumingMemoryBound(to: UInt8.self) else {
                return false
            }
            return clipsync_push_local(eng, kind.rawValue, ptr, data.count) == 1
        }
    }

    func popRemote() -> RemoteClipItem? {
        guard let eng = engine else { return nil }
        var item = ClipSyncItem()
        let rc = clipsync_pop_remote(eng, &item)
        guard rc == 1 else { return nil }
        defer { clipsync_free_item(&item) }

        guard let kind = ClipSyncKind(rawValue: item.kind),
              let ptr = item.data,
              item.data_len > 0
        else {
            return nil
        }
        let data = Data(bytes: ptr, count: item.data_len)
        let mime = item.mime.map { String(cString: $0) } ?? "application/octet-stream"
        return RemoteClipItem(
            kind: kind,
            data: data,
            mime: mime,
            timestampMs: item.timestamp_ms
        )
    }

    @discardableResult
    func pinItem(kind: ClipSyncKind, data: Data) -> Data? {
        guard let eng = engine, !data.isEmpty else { return nil }
        var outId = [UInt8](repeating: 0, count: 16)
        let rc: Int32 = data.withUnsafeBytes { raw in
            guard let ptr = raw.baseAddress?.assumingMemoryBound(to: UInt8.self) else {
                return -1
            }
            return outId.withUnsafeMutableBufferPointer { idBuf in
                clipsync_pin_item(eng, kind.rawValue, ptr, data.count, idBuf.baseAddress)
            }
        }
        return rc == 0 ? Data(outId) : nil
    }

    @discardableResult
    func unpinItem(id: Data) -> Bool {
        guard let eng = engine, id.count == 16 else { return false }
        return id.withUnsafeBytes { raw in
            guard let ptr = raw.baseAddress?.assumingMemoryBound(to: UInt8.self) else {
                return false
            }
            return clipsync_unpin_item(eng, ptr) == 1
        }
    }

    func getPins() -> [PinnedClipItem] {
        guard let eng = engine else { return [] }
        var list = ClipSyncPinList(items: nil, count: 0)
        guard clipsync_get_pins(eng, &list) == 0 else { return [] }
        defer { clipsync_free_pin_list(&list) }

        guard let itemsPtr = list.items, list.count > 0 else { return [] }
        var result: [PinnedClipItem] = []
        result.reserveCapacity(list.count)
        for i in 0..<list.count {
            let cPin = itemsPtr[i]
            guard let kind = ClipSyncKind(rawValue: cPin.kind),
                  let dataPtr = cPin.data
            else { continue }
            let idData = withUnsafeBytes(of: cPin.id) { Data($0) }
            let data = Data(bytes: dataPtr, count: cPin.data_len)
            let mime = cPin.mime.map { String(cString: $0) } ?? "application/octet-stream"
            result.append(
                PinnedClipItem(
                    id: idData,
                    kind: kind,
                    data: data,
                    mime: mime,
                    createdAtMs: cPin.created_at_ms,
                    updatedAtMs: cPin.updated_at_ms
                )
            )
        }
        return result
    }

    func getPairingUri() -> String {
        guard let eng = engine, let cStr = clipsync_get_pairing_uri(eng) else {
            return ""
        }
        defer { clipsync_free_string(cStr) }
        return String(cString: cStr)
    }

    @discardableResult
    func joinPairingUri(_ uri: String) -> Bool {
        guard let eng = engine else { return false }
        return uri.withCString { cStr in
            clipsync_join_pairing_uri(eng, cStr) == 0
        }
    }

    func recordEcho(kind: ClipSyncKind, data: Data) {
        guard let eng = engine, !data.isEmpty else { return }
        data.withUnsafeBytes { raw in
            if let ptr = raw.baseAddress?.assumingMemoryBound(to: UInt8.self) {
                clipsync_record_echo(eng, kind.rawValue, ptr, data.count)
            }
        }
    }
}
