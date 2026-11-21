import AppKit
import CoreImage.CIFilterBuiltins
import SwiftUI

struct PopoverView: View {
    @ObservedObject var monitor: PasteboardMonitor
    @State private var joinInput: String = ""
    @State private var showPairing: Bool = false

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Label("ClipSync", systemImage: "doc.on.clipboard")
                    .font(.headline)
                Spacer()
                Button(showPairing ? "Pins" : "Pair") {
                    showPairing.toggle()
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
            }

            if showPairing {
                pairingSection
            } else {
                pinsSection
            }

            Divider()

            Toggle(
                "Start at Login",
                isOn: Binding(
                    get: { monitor.launchAtLogin },
                    set: { monitor.setLaunchAtLogin($0) }
                )
            )
            .toggleStyle(.switch)
            .controlSize(.small)
            .font(.caption)

            HStack {
                Text(monitor.statusMessage)
                    .font(.caption2)
                    .foregroundColor(.secondary)
                    .lineLimit(1)
                Spacer()
                Button("Quit") {
                    NSApp.terminate(nil)
                }
                .buttonStyle(.borderless)
                .font(.caption)
            }
        }
        .padding(12)
        .frame(width: 320, height: 420)
    }

    private var pinsSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            Button {
                monitor.pinCurrentClipboard()
            } label: {
                Label("Pin Current Clipboard", systemImage: "pin.fill")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent)

            if monitor.pins.isEmpty {
                Spacer()
                Text("No pinned items yet.\nCopy text or an image and click Pin.")
                    .font(.caption)
                    .foregroundColor(.secondary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: .infinity)
                Spacer()
            } else {
                ScrollView {
                    LazyVStack(spacing: 6) {
                        ForEach(monitor.pins) { pin in
                            PinRowView(
                                pin: pin,
                                onSelect: { monitor.copyPinToClipboard(pin) },
                                onDelete: { monitor.unpin(pin) }
                            )
                        }
                    }
                }
            }
        }
    }

    private var pairingSection: some View {
        VStack(spacing: 10) {
            if let qrImage = generateQRCode(from: monitor.pairingUri) {
                Image(nsImage: qrImage)
                    .interpolation(.none)
                    .resizable()
                    .scaledToFit()
                    .frame(width: 176, height: 176)
                    .background(Color.white)
                    .cornerRadius(8)
            }

            HStack(spacing: 6) {
                Text(monitor.pairingUri)
                    .font(.system(.caption2, design: .monospaced))
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .frame(maxWidth: .infinity, alignment: .leading)
                Button("Copy") {
                    monitor.copyPairingUri()
                }
                .controlSize(.small)
            }

            Divider()

            VStack(alignment: .leading, spacing: 6) {
                Text("Join Existing Cluster")
                    .font(.caption.weight(.semibold))
                HStack(spacing: 6) {
                    TextField("clipsync://pair?k=...&r=...", text: $joinInput)
                        .textFieldStyle(.roundedBorder)
                        .font(.caption)
                    Button("Join") {
                        if monitor.joinCluster(uri: joinInput) {
                            joinInput = ""
                        }
                    }
                    .controlSize(.small)
                    .disabled(joinInput.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
            }
            Spacer()
        }
    }

    private func generateQRCode(from text: String) -> NSImage? {
        guard !text.isEmpty else { return nil }
        let context = CIContext()
        let filter = CIFilter.qrCodeGenerator()
        filter.message = Data(text.utf8)
        filter.correctionLevel = "M"

        guard let outputImage = filter.outputImage else { return nil }
        let transform = CGAffineTransform(scaleX: 10, y: 10)
        let scaled = outputImage.transformed(by: transform)
        guard let cgImage = context.createCGImage(scaled, from: scaled.extent) else { return nil }
        return NSImage(cgImage: cgImage, size: NSSize(width: scaled.extent.width, height: scaled.extent.height))
    }
}

private struct PinRowView: View {
    let pin: PinnedClipItem
    let onSelect: () -> Void
    let onDelete: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            Button(action: onSelect) {
                HStack(spacing: 8) {
                    if pin.kind == .image, let img = NSImage(data: pin.data) {
                        Image(nsImage: img)
                            .resizable()
                            .scaledToFill()
                            .frame(width: 34, height: 34)
                            .clipped()
                            .cornerRadius(4)
                        Text("Image (\(pin.data.count / 1024) KB)")
                            .font(.caption)
                            .foregroundColor(.primary)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    } else {
                        let text = String(data: pin.data, encoding: .utf8) ?? "<binary>"
                        Text(text.replacingOccurrences(of: "\n", with: " "))
                            .font(.caption)
                            .foregroundColor(.primary)
                            .lineLimit(2)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            Button(action: onDelete) {
                Image(systemName: "trash")
                    .font(.caption)
                    .foregroundColor(.secondary)
            }
            .buttonStyle(.borderless)
            .help("Unpin")
        }
        .padding(8)
        .background(Color(NSColor.controlBackgroundColor))
        .cornerRadius(6)
    }
}
