// Camera Center virtual camera (CoreMediaIO Camera Extension).
//
// The device exposes two streams:
//  * a *source* stream that video-call apps open ("Camera Center"),
//  * a *sink* stream that the Camera Center app feeds with processed frames.
// Frames received on the sink are re-timed and forwarded to the source. When
// the app isn't running, a placeholder frame is shown.
//
// Keep the constants below in sync with crates/cc-virtualcam/src/macos/mod.rs.

import CoreMediaIO
import CoreText
import Foundation
import IOKit.audio
import os.log

let kDeviceName = "Camera Center"
let kDeviceUID = UUID(uuidString: "5B6A2F0C-7C1E-4D8B-9A51-3C0E2B7D4F61")!
let kSourceStreamUID = UUID(uuidString: "0E3C9D52-8A41-4F7B-B2D6-61A5C4E8F903")!
let kSinkStreamUID = UUID(uuidString: "A7F21B64-3D9E-4C08-8E5A-2B7C9F1D0E46")!
let kWidth: Int32 = 1920
let kHeight: Int32 = 1080
let kFrameRate: Int32 = 30
/// Sink frames older than this are considered stale (app quit or paused).
let kSinkTimeoutSeconds: Double = 1.0

private let log = Logger(subsystem: "com.cameracenter.app.camera-extension", category: "camera")

// MARK: - Device

final class CameraDeviceSource: NSObject, CMIOExtensionDeviceSource {
    private(set) var device: CMIOExtensionDevice!
    private var sourceStream: CameraStreamSource!
    private var sinkStream: CameraStreamSource!

    private var videoDescription: CMFormatDescription!
    private var bufferPool: CVPixelBufferPool!
    private var bufferAuxAttributes: NSDictionary!
    private var placeholder: CVPixelBuffer?

    private let timerQueue = DispatchQueue(label: "camera-center.timer", qos: .userInteractive)
    private var timer: DispatchSourceTimer?
    private var sourceClients = 0

    private let lock = NSLock()
    private var latestSinkBuffer: CVPixelBuffer?
    private var latestSinkTime: Double = 0
    private var sinkStreaming = false
    private var sinkClient: CMIOExtensionClient?

    init(localizedName: String) {
        super.init()
        device = CMIOExtensionDevice(
            localizedName: localizedName,
            deviceID: kDeviceUID,
            legacyDeviceID: kDeviceUID.uuidString,
            source: self
        )

        CMVideoFormatDescriptionCreate(
            allocator: kCFAllocatorDefault,
            codecType: kCVPixelFormatType_32BGRA,
            width: kWidth,
            height: kHeight,
            extensions: nil,
            formatDescriptionOut: &videoDescription
        )
        let pixelBufferAttributes: NSDictionary = [
            kCVPixelBufferWidthKey: kWidth,
            kCVPixelBufferHeightKey: kHeight,
            kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_32BGRA,
            kCVPixelBufferIOSurfacePropertiesKey: [:] as NSDictionary,
        ]
        CVPixelBufferPoolCreate(kCFAllocatorDefault, nil, pixelBufferAttributes, &bufferPool)
        bufferAuxAttributes = [kCVPixelBufferPoolAllocationThresholdKey: 5]

        let frameDuration = CMTime(value: 1, timescale: kFrameRate)
        let streamFormat = CMIOExtensionStreamFormat(
            formatDescription: videoDescription,
            maxFrameDuration: frameDuration,
            minFrameDuration: frameDuration,
            validFrameDurations: nil
        )
        // Order matters: the app picks the sink as the second stream.
        sourceStream = CameraStreamSource(
            localizedName: "\(kDeviceName).Video",
            streamID: kSourceStreamUID,
            streamFormat: streamFormat,
            direction: .source,
            owner: self
        )
        sinkStream = CameraStreamSource(
            localizedName: "\(kDeviceName).Sink",
            streamID: kSinkStreamUID,
            streamFormat: streamFormat,
            direction: .sink,
            owner: self
        )
        do {
            try device.addStream(sourceStream.stream)
            try device.addStream(sinkStream.stream)
        } catch {
            fatalError("Failed to add streams: \(error.localizedDescription)")
        }
        placeholder = makePlaceholder()
    }

    var availableProperties: Set<CMIOExtensionProperty> {
        [.deviceTransportType, .deviceModel]
    }

    func deviceProperties(forProperties properties: Set<CMIOExtensionProperty>) throws
        -> CMIOExtensionDeviceProperties
    {
        let deviceProperties = CMIOExtensionDeviceProperties(dictionary: [:])
        if properties.contains(.deviceTransportType) {
            deviceProperties.transportType = kIOAudioDeviceTransportTypeVirtual
        }
        if properties.contains(.deviceModel) {
            deviceProperties.model = kDeviceName
        }
        return deviceProperties
    }

    func setDeviceProperties(_ deviceProperties: CMIOExtensionDeviceProperties) throws {}

    // MARK: Source stream (consumed by video apps)

    func startStreaming() {
        timerQueue.async { [self] in
            sourceClients += 1
            guard timer == nil else { return }
            let t = DispatchSource.makeTimerSource(flags: .strict, queue: timerQueue)
            t.schedule(deadline: .now(), repeating: 1.0 / Double(kFrameRate), leeway: .milliseconds(2))
            t.setEventHandler { [weak self] in self?.emitFrame() }
            t.resume()
            timer = t
            log.info("source stream started")
        }
    }

    func stopStreaming() {
        timerQueue.async { [self] in
            sourceClients = max(0, sourceClients - 1)
            if sourceClients == 0 {
                timer?.cancel()
                timer = nil
                log.info("source stream stopped")
            }
        }
    }

    private func emitFrame() {
        let now = CMClockGetTime(CMClockGetHostTimeClock())
        lock.lock()
        let fresh = latestSinkBuffer.flatMap { buffer in
            now.seconds - latestSinkTime < kSinkTimeoutSeconds ? buffer : nil
        }
        lock.unlock()
        guard let pixelBuffer = fresh ?? placeholder else { return }

        var format: CMFormatDescription?
        CMVideoFormatDescriptionCreateForImageBuffer(
            allocator: kCFAllocatorDefault, imageBuffer: pixelBuffer, formatDescriptionOut: &format)
        guard let format else { return }
        var timing = CMSampleTimingInfo(
            duration: CMTime(value: 1, timescale: kFrameRate),
            presentationTimeStamp: now,
            decodeTimeStamp: .invalid
        )
        var sampleBuffer: CMSampleBuffer?
        let err = CMSampleBufferCreateForImageBuffer(
            allocator: kCFAllocatorDefault,
            imageBuffer: pixelBuffer,
            dataReady: true,
            makeDataReadyCallback: nil,
            refcon: nil,
            formatDescription: format,
            sampleTiming: &timing,
            sampleBufferOut: &sampleBuffer
        )
        guard err == noErr, let sampleBuffer else {
            log.error("CMSampleBufferCreateForImageBuffer failed: \(err)")
            return
        }
        sourceStream.stream.send(
            sampleBuffer,
            discontinuity: [],
            hostTimeInNanoseconds: UInt64(now.seconds * Double(NSEC_PER_SEC))
        )
    }

    // MARK: Sink stream (fed by the Camera Center app)

    func startSinkStreaming(client: CMIOExtensionClient) {
        lock.lock()
        sinkClient = client
        sinkStreaming = true
        lock.unlock()
        log.info("sink stream started")
        consumeBuffer(client)
    }

    func stopSinkStreaming() {
        lock.lock()
        sinkStreaming = false
        sinkClient = nil
        latestSinkBuffer = nil
        lock.unlock()
        log.info("sink stream stopped")
    }

    private func consumeBuffer(_ client: CMIOExtensionClient) {
        lock.lock()
        let streaming = sinkStreaming
        lock.unlock()
        guard streaming else { return }

        sinkStream.stream.consumeSampleBuffer(from: client) { [weak self] sampleBuffer, sequence, _, _, error in
            guard let self else { return }
            if let sampleBuffer, let pixelBuffer = CMSampleBufferGetImageBuffer(sampleBuffer) {
                let now = CMClockGetTime(CMClockGetHostTimeClock())
                self.lock.lock()
                self.latestSinkBuffer = pixelBuffer
                self.latestSinkTime = now.seconds
                self.lock.unlock()
                let output = CMIOExtensionScheduledOutput(
                    sequenceNumber: sequence,
                    hostTimeInNanoseconds: UInt64(now.seconds * Double(NSEC_PER_SEC))
                )
                self.sinkStream.stream.notifyScheduledOutputChanged(output)
            } else if let error {
                log.debug("consumeSampleBuffer: \(error.localizedDescription)")
            }
            self.consumeBuffer(client)
        }
    }

    // MARK: Placeholder

    private func makePlaceholder() -> CVPixelBuffer? {
        var pixelBuffer: CVPixelBuffer?
        CVPixelBufferPoolCreatePixelBufferWithAuxAttributes(
            kCFAllocatorDefault, bufferPool, bufferAuxAttributes, &pixelBuffer)
        guard let pixelBuffer else { return nil }
        CVPixelBufferLockBaseAddress(pixelBuffer, [])
        defer { CVPixelBufferUnlockBaseAddress(pixelBuffer, []) }
        guard
            let context = CGContext(
                data: CVPixelBufferGetBaseAddress(pixelBuffer),
                width: Int(kWidth),
                height: Int(kHeight),
                bitsPerComponent: 8,
                bytesPerRow: CVPixelBufferGetBytesPerRow(pixelBuffer),
                space: CGColorSpaceCreateDeviceRGB(),
                bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue
                    | CGBitmapInfo.byteOrder32Little.rawValue
            )
        else { return pixelBuffer }
        context.setFillColor(CGColor(red: 0.07, green: 0.07, blue: 0.09, alpha: 1))
        context.fill(CGRect(x: 0, y: 0, width: Int(kWidth), height: Int(kHeight)))

        let text = "Open Camera Center to start the virtual camera" as CFString
        let font = CTFontCreateWithName("Helvetica Neue" as CFString, 44, nil)
        let attributes = [
            kCTFontAttributeName: font,
            kCTForegroundColorAttributeName: CGColor(red: 0.8, green: 0.8, blue: 0.85, alpha: 1),
        ] as CFDictionary
        let line = CTLineCreateWithAttributedString(
            CFAttributedStringCreate(nil, text, attributes))
        let bounds = CTLineGetBoundsWithOptions(line, [])
        context.textPosition = CGPoint(
            x: (CGFloat(kWidth) - bounds.width) / 2,
            y: (CGFloat(kHeight) - bounds.height) / 2
        )
        CTLineDraw(line, context)
        return pixelBuffer
    }
}

// MARK: - Streams

final class CameraStreamSource: NSObject, CMIOExtensionStreamSource {
    private(set) var stream: CMIOExtensionStream!
    private let streamFormat: CMIOExtensionStreamFormat
    private let direction: CMIOExtensionStream.Direction
    private weak var owner: CameraDeviceSource?
    private var client: CMIOExtensionClient?

    init(
        localizedName: String,
        streamID: UUID,
        streamFormat: CMIOExtensionStreamFormat,
        direction: CMIOExtensionStream.Direction,
        owner: CameraDeviceSource
    ) {
        self.streamFormat = streamFormat
        self.direction = direction
        self.owner = owner
        super.init()
        stream = CMIOExtensionStream(
            localizedName: localizedName,
            streamID: streamID,
            direction: direction,
            clockType: .hostTime,
            source: self
        )
    }

    var formats: [CMIOExtensionStreamFormat] { [streamFormat] }

    var activeFormatIndex: Int = 0 {
        didSet {
            if activeFormatIndex >= 1 { log.error("invalid format index \(self.activeFormatIndex)") }
        }
    }

    var availableProperties: Set<CMIOExtensionProperty> {
        var properties: Set<CMIOExtensionProperty> = [.streamActiveFormatIndex, .streamFrameDuration]
        if direction == .sink {
            properties.formUnion([
                .streamSinkBufferQueueSize,
                .streamSinkBuffersRequiredForStartup,
                .streamSinkBufferUnderrunCount,
                .streamSinkEndOfData,
            ])
        }
        return properties
    }

    func streamProperties(forProperties properties: Set<CMIOExtensionProperty>) throws
        -> CMIOExtensionStreamProperties
    {
        let streamProperties = CMIOExtensionStreamProperties(dictionary: [:])
        if properties.contains(.streamActiveFormatIndex) {
            streamProperties.activeFormatIndex = 0
        }
        if properties.contains(.streamFrameDuration) {
            streamProperties.frameDuration = CMTime(value: 1, timescale: kFrameRate)
        }
        if properties.contains(.streamSinkBufferQueueSize) {
            streamProperties.sinkBufferQueueSize = 2
        }
        if properties.contains(.streamSinkBuffersRequiredForStartup) {
            streamProperties.sinkBuffersRequiredForStartup = 1
        }
        return streamProperties
    }

    func setStreamProperties(_ streamProperties: CMIOExtensionStreamProperties) throws {
        if let index = streamProperties.activeFormatIndex {
            activeFormatIndex = index
        }
    }

    func authorizedToStartStream(for client: CMIOExtensionClient) -> Bool {
        self.client = client
        return true
    }

    func startStream() throws {
        guard let owner else { return }
        if direction == .sink {
            guard let client else { return }
            owner.startSinkStreaming(client: client)
        } else {
            owner.startStreaming()
        }
    }

    func stopStream() throws {
        guard let owner else { return }
        if direction == .sink {
            owner.stopSinkStreaming()
        } else {
            owner.stopStreaming()
        }
    }
}

// MARK: - Provider

final class CameraProviderSource: NSObject, CMIOExtensionProviderSource {
    private(set) var provider: CMIOExtensionProvider!
    private var deviceSource: CameraDeviceSource!

    init(clientQueue: DispatchQueue?) {
        super.init()
        provider = CMIOExtensionProvider(source: self, clientQueue: clientQueue)
        deviceSource = CameraDeviceSource(localizedName: kDeviceName)
        do {
            try provider.addDevice(deviceSource.device)
        } catch {
            fatalError("Failed to add device: \(error.localizedDescription)")
        }
    }

    func connect(to client: CMIOExtensionClient) throws {}

    func disconnect(from client: CMIOExtensionClient) {}

    var availableProperties: Set<CMIOExtensionProperty> {
        [.providerManufacturer]
    }

    func providerProperties(forProperties properties: Set<CMIOExtensionProperty>) throws
        -> CMIOExtensionProviderProperties
    {
        let providerProperties = CMIOExtensionProviderProperties(dictionary: [:])
        if properties.contains(.providerManufacturer) {
            providerProperties.manufacturer = kDeviceName
        }
        return providerProperties
    }

    func setProviderProperties(_ providerProperties: CMIOExtensionProviderProperties) throws {}
}
