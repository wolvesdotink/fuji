import Foundation
import ImageCaptureCore
import CoreGraphics
import ImageIO
import UniformTypeIdentifiers

// MARK: - JSON Output Helpers

func printJSON(_ value: Any) {
    if let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]),
       let str = String(data: data, encoding: .utf8) {
        print(str)
    }
}

func printError(_ message: String) {
    let err: [String: Any] = ["error": message]
    printJSON(err)
}

func stderrLog(_ message: String) {
    FileHandle.standardError.write("[ptp-bridge] \(message)\n".data(using: .utf8)!)
}

func saveCGImageAsJPEG(_ cgImage: CGImage, to url: URL) -> Bool {
    guard let dest = CGImageDestinationCreateWithURL(url as CFURL, UTType.jpeg.identifier as CFString, 1, nil) else {
        return false
    }
    CGImageDestinationAddImage(dest, cgImage, [kCGImageDestinationLossyCompressionQuality: 0.85] as CFDictionary)
    return CGImageDestinationFinalize(dest)
}

// MARK: - Daemon Tuning

/// How long a camera session may sit unused before the daemon closes it, so the
/// camera is not locked forever and can go to sleep, plus how often we look.
let daemonIdleTimeout: TimeInterval = 60.0
let daemonIdleCheckInterval: TimeInterval = 15.0

/// Minimum spacing between byte-progress lines. Unthrottled, a fast transfer
/// emits thousands of NDJSON lines that all end up as webview IPC messages.
let downloadProgressInterval: TimeInterval = 0.1

// MARK: - PTP Bridge

class PtpBridge: NSObject, ICDeviceBrowserDelegate, ICCameraDeviceDelegate, ICCameraDeviceDownloadDelegate {
    let browser = ICDeviceBrowser()
    var discoveredCameras: [ICCameraDevice] = []

    // Session state
    var sessionOpened = false
    var sessionError: Error?
    var sessionCloseDone = false
    var sessionCloseError: Error?

    // Catalog state
    var catalogDone = false

    // Thumbnail state
    var pendingThumbnails = 0
    var thumbnailResults: [String: CGImage] = [:]

    // Download state
    var downloadDone = false
    var downloadError: Error?
    var downloadedURL: URL?

    // Delete state
    var deleteDone = false
    var deleteError: Error?

    // Daemon-mode state. When the binary runs as `ptp-bridge daemon`, the
    // browser starts once at init and stays alive for the life of the process.
    // `daemonCameras` is kept in sync via didAdd/didRemove callbacks, so
    // lookups never race a fresh discovery window.
    var daemonMode = false
    var daemonCameras: [String: ICCameraDevice] = [:]

    // Sessions and catalogs are held across daemon requests: opening a session
    // costs seconds and a full-card catalog wait costs ~45s, and paying that per
    // request meant every single-file preview download during culling re-paid it.
    // Keyed by object identity because serialNumberString is unreliable once a
    // device starts going away (same reason didRemove matches by identity).
    var daemonSessions: Set<ObjectIdentifier> = []
    var daemonCatalogReady: Set<ObjectIdentifier> = []

    /// Nesting depth of in-flight daemon requests. A depth > 0 means the main
    /// thread is inside a command's nested run loop, so the idle timer must not
    /// pull the session out from under it. It is a depth rather than a flag
    /// because out-of-band commands (`cancel`) run re-entrantly.
    var daemonBusyDepth = 0
    var daemonLastActivity = Date()
    var daemonIdleTimer: Timer?

    // Byte progress for the download request currently in flight. `bytesBase` is
    // the summed size of the files that already finished, so base + the current
    // file's partial count is the batch-cumulative figure the UI turns into a
    // rate and ETA.
    var downloadProgressId: Int?
    var downloadBytesBase: Int64 = 0
    var downloadBytesTotal: Int64 = 0
    var downloadCurrentBytes: Int64 = 0
    var downloadFilesCompleted = 0
    var downloadFilesTotal = 0
    var downloadCurrentName = ""
    var lastProgressEmit = Date.distantPast
    var downloadCancelled = false

    override init() {
        super.init()
        browser.delegate = self
    }

    // MARK: - Device Discovery (one-shot mode)

    func discoverCameras(timeout: TimeInterval = 10.0, keepBrowsing: Bool = false) -> [ICCameraDevice] {
        discoveredCameras = []
        stderrLog("Starting PTP discovery (timeout=\(timeout)s)")
        browser.start()

        let deadline = Date(timeIntervalSinceNow: timeout)
        while Date() < deadline {
            RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.1))
            if !discoveredCameras.isEmpty {
                // Give a moment for additional cameras
                RunLoop.main.run(until: Date(timeIntervalSinceNow: 1.0))
                break
            }
        }

        if !keepBrowsing {
            browser.stop()
        }
        stderrLog("Discovery complete. Found \(discoveredCameras.count) camera(s)")
        return discoveredCameras
    }

    /// Find a camera by name and set its delegate immediately.
    /// Keeps the browser running so the device reference stays valid.
    func findCamera(name: String, timeout: TimeInterval = 5.0) -> ICCameraDevice? {
        let cameras = discoverCameras(timeout: timeout, keepBrowsing: true)

        var result: ICCameraDevice? = nil

        // Try exact match first
        if let camera = cameras.first(where: { ($0.name ?? "") == name }) {
            result = camera
        }
        // Try case-insensitive contains
        else if let camera = cameras.first(where: { ($0.name ?? "").localizedCaseInsensitiveContains(name) }) {
            result = camera
        }
        // Return first camera if only one found
        else if cameras.count == 1 {
            result = cameras.first
        }

        // Set delegate immediately on discovered device
        if let cam = result {
            cam.delegate = self
        }

        return result
    }

    func stopBrowsing() {
        browser.stop()
    }

    // MARK: - Session Management

    /// Spin the main run loop until `isDone` or the deadline passes.
    ///
    /// ImageCaptureCore delivers everything through main-thread delegate
    /// callbacks, so we have to drain the run loop while waiting. Those
    /// callbacks call `CFRunLoopStop(CFRunLoopGetMain())`, which returns from the
    /// slice the moment the event lands instead of on the next tick. The slice is
    /// kept short anyway as a backstop, since it also bounds how long a wait
    /// overshoots its own deadline.
    func waitForCallback(timeout: TimeInterval, isDone: () -> Bool) -> Bool {
        let deadline = Date(timeIntervalSinceNow: timeout)
        while !isDone() && Date() < deadline {
            RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.05))
        }
        return isDone()
    }

    func openSession(camera: ICCameraDevice, timeout: TimeInterval = 10.0) -> Bool {
        sessionOpened = false
        sessionError = nil
        catalogDone = false
        camera.delegate = self
        camera.requestOpenSession()

        _ = waitForCallback(timeout: timeout) { sessionOpened }

        return sessionOpened && sessionError == nil
    }

    func waitForCatalog(camera: ICCameraDevice, timeout: TimeInterval = 60.0) -> Bool {
        catalogDone = false

        let deadline = Date(timeIntervalSinceNow: timeout)
        var lastPercent: Int = -1
        while !catalogDone && Date() < deadline {
            RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.25))

            // Poll contentCatalogPercentCompleted as a fallback
            let pct = camera.contentCatalogPercentCompleted
            if pct != lastPercent {
                stderrLog("Catalog progress: \(pct)%")
                lastPercent = pct
            }

            // If we have files and percent is 100, consider it done
            if pct >= 100 && (camera.mediaFiles?.count ?? 0) > 0 {
                stderrLog("Catalog complete (via polling). Files: \(camera.mediaFiles?.count ?? 0)")
                catalogDone = true
            }
        }

        return catalogDone
    }

    /// Closing an ImageCaptureCore session is asynchronous. Daemon commands
    /// must wait for it before replying, otherwise a delete issued immediately
    /// after an import races the prior download session's close and is ignored
    /// by some Fuji bodies.
    func closeSession(camera: ICCameraDevice, timeout: TimeInterval = 10.0) -> Bool {
        sessionCloseDone = false
        sessionCloseError = nil
        camera.requestCloseSession()

        _ = waitForCallback(timeout: timeout) { sessionCloseDone }

        if !sessionCloseDone {
            stderrLog("Session close timed out")
            return false
        }
        if let error = sessionCloseError {
            stderrLog("Session close error: \(error.localizedDescription)")
            return false
        }
        return true
    }

    // MARK: - Persistent Daemon Sessions

    /// Open a session only if we do not already hold one. `hasOpenSession` is
    /// consulted as well as our own bookkeeping so a session the camera dropped
    /// behind our back (sleep, cable glitch) is reopened rather than used.
    func ensureSession(camera: ICCameraDevice) -> Bool {
        // Original assets, so RAW+HEIF pairs show up instead of rendered
        // previews. Assigned only when it differs, because changing the
        // presentation re-enumerates the device content and would invalidate the
        // catalog readiness cached below.
        if camera.mediaPresentation != .originalAssets {
            camera.mediaPresentation = .originalAssets
        }

        let key = ObjectIdentifier(camera)
        if daemonSessions.contains(key) && camera.hasOpenSession {
            return true
        }
        if daemonSessions.contains(key) {
            stderrLog("Daemon: session on '\(camera.name ?? "?")' was dropped, reopening")
            daemonSessions.remove(key)
            daemonCatalogReady.remove(key)
        }
        guard openSession(camera: camera) else { return false }
        daemonSessions.insert(key)
        return true
    }

    /// Wait for the content catalog unless this session already completed one.
    /// The first interaction after connect pays the wait; everything after it
    /// starts transferring immediately.
    func ensureCatalog(camera: ICCameraDevice, timeout: TimeInterval = 60.0) -> Bool {
        let key = ObjectIdentifier(camera)
        if daemonCatalogReady.contains(key) {
            return true
        }
        guard waitForCatalog(camera: camera, timeout: timeout) else { return false }
        daemonCatalogReady.insert(key)
        return true
    }

    /// Explicitly give a camera back. Only the idle timer, shutdown and failed
    /// requests do this — successful requests leave the session open.
    func releaseSession(camera: ICCameraDevice) {
        let key = ObjectIdentifier(camera)
        daemonSessions.remove(key)
        daemonCatalogReady.remove(key)
        _ = closeSession(camera: camera)
    }

    /// Drop session bookkeeping for a camera that is already gone. Closing a
    /// session on an unplugged device is pointless and would just time out.
    func forgetSession(camera: ICCameraDevice) {
        let key = ObjectIdentifier(camera)
        daemonSessions.remove(key)
        daemonCatalogReady.remove(key)
    }

    func closeAllDaemonSessions() {
        for camera in daemonCameras.values where daemonSessions.contains(ObjectIdentifier(camera)) {
            stderrLog("Daemon: closing session on '\(camera.name ?? "?")'")
            releaseSession(camera: camera)
        }
    }

    /// Idle-timer tick. Skipped while a request is in flight, since closing the
    /// session mid-download would abort it.
    func closeIdleSessions() {
        guard daemonBusyDepth == 0, !daemonSessions.isEmpty else { return }
        guard Date().timeIntervalSince(daemonLastActivity) >= daemonIdleTimeout else { return }
        stderrLog("Daemon: idle for \(Int(daemonIdleTimeout))s, releasing camera session(s)")
        closeAllDaemonSessions()
    }

    // MARK: - Thumbnail Requests

    func isPreviewableMedia(_ name: String) -> Bool {
        let ext = (name as NSString).pathExtension.uppercased()
        return ["HIF", "HEIF", "HEIC", "JPG", "JPEG", "MOV", "MP4", "M4V", "AVI"].contains(ext)
    }

    func thumbnailCacheId(_ name: String) -> String {
        let ext = (name as NSString).pathExtension.uppercased()
        // Movie ids include their extension on the Rust/TypeScript side so a
        // still and movie with the same stem cannot overwrite each other.
        if ["MOV", "MP4", "M4V", "AVI"].contains(ext) {
            return name
        }
        return (name as NSString).deletingPathExtension
    }

    func requestAllThumbnails(camera: ICCameraDevice, items: [ICCameraItem], timeout: TimeInterval = 120.0) {
        thumbnailResults = [:]
        pendingThumbnails = items.count

        for item in items {
            item.requestThumbnail()
        }

        let deadline = Date(timeIntervalSinceNow: timeout)
        while pendingThumbnails > 0 && Date() < deadline {
            RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.1))
        }
    }

    // MARK: - File Download

    func downloadFile(camera: ICCameraDevice, file: ICCameraFile, destDir: URL, timeout: TimeInterval = 300.0) -> URL? {
        downloadDone = false
        downloadError = nil
        downloadedURL = nil

        let options: [ICDownloadOption: Any] = [
            .downloadsDirectoryURL: destDir,
            .overwrite: true
        ]

        camera.requestDownloadFile(file, options: options, downloadDelegate: self, didDownloadSelector: #selector(didDownloadFile(_:error:options:contextInfo:)), contextInfo: nil)

        _ = waitForCallback(timeout: timeout) { downloadDone }

        if let error = downloadError {
            stderrLog("Download error: \(error.localizedDescription)")
            return nil
        }

        return downloadedURL
    }

    // MARK: - File Delete

    func deleteFiles(camera: ICCameraDevice, files: [ICCameraItem], timeout: TimeInterval = 60.0) -> String? {
        deleteDone = false
        deleteError = nil

        camera.requestDeleteFiles(files)

        _ = waitForCallback(timeout: timeout) { deleteDone }

        // The old implementation returned nil here even when the delegate
        // callback never arrived, causing the UI to announce a successful
        // deletion while every file remained on the card.
        if !deleteDone {
            return "Timed out waiting for the camera to confirm deletion"
        }
        return deleteError?.localizedDescription
    }

    // MARK: - ICDeviceBrowserDelegate

    func deviceBrowser(_ browser: ICDeviceBrowser, didAdd device: ICDevice, moreComing: Bool) {
        if let camera = device as? ICCameraDevice {
            stderrLog("Discovered camera: \(camera.name ?? "unknown") transport=\(camera.transportType ?? "?")")
            discoveredCameras.append(camera)

            if daemonMode {
                // Daemon keeps a live registry so commands don't have to
                // rediscover. Key by serial if we have one, else by name.
                let key = (camera.serialNumberString?.isEmpty == false)
                    ? camera.serialNumberString!
                    : (camera.name ?? "unknown-\(daemonCameras.count)")
                daemonCameras[key] = camera
                camera.delegate = self
                stderrLog("Daemon: registered '\(camera.name ?? "?")' key=\(key) (total=\(daemonCameras.count))")
            }
        }
    }

    func deviceBrowser(_ browser: ICDeviceBrowser, didRemove device: ICDevice, moreGoing: Bool) {
        if let camera = device as? ICCameraDevice {
            stderrLog("Camera removed: \(camera.name ?? "unknown")")
            if daemonMode {
                // Match by object identity — serialNumberString can return nil
                // after removal begins, so key-based lookup is unreliable here.
                daemonCameras = daemonCameras.filter { _, cam in cam !== camera }
                forgetSession(camera: camera)
                stderrLog("Daemon: registry size after remove = \(daemonCameras.count)")
            }
        }
    }

    // MARK: - ICDeviceDelegate

    func device(_ device: ICDevice, didOpenSessionWithError error: (any Error)?) {
        sessionError = error
        sessionOpened = true
        if let error = error {
            stderrLog("Session open error: \(error.localizedDescription)")
        } else {
            stderrLog("Session opened successfully")
        }
        CFRunLoopStop(CFRunLoopGetMain())
    }

    func device(_ device: ICDevice, didCloseSessionWithError error: (any Error)?) {
        sessionCloseError = error
        sessionCloseDone = true
        CFRunLoopStop(CFRunLoopGetMain())
    }

    func didRemove(_ device: ICDevice) {}

    // MARK: - ICCameraDeviceDelegate (required methods)

    func cameraDevice(_ camera: ICCameraDevice, didAdd items: [ICCameraItem]) {
        stderrLog("didAdd \(items.count) items (total so far: \(camera.mediaFiles?.count ?? 0))")
    }

    func cameraDevice(_ camera: ICCameraDevice, didRemove items: [ICCameraItem]) {}

    func cameraDevice(_ camera: ICCameraDevice, didRenameItems items: [ICCameraItem]) {}

    func cameraDevice(_ camera: ICCameraDevice, didReceiveMetadata metadata: [AnyHashable: Any]?, for item: ICCameraItem, error: (any Error)?) {}

    func cameraDeviceDidChangeCapability(_ camera: ICCameraDevice) {}

    func cameraDevice(_ camera: ICCameraDevice, didReceivePTPEvent eventData: Data) {}

    func deviceDidBecomeReady(withCompleteContentCatalog device: ICCameraDevice) {
        stderrLog("Content catalog complete. Files: \(device.mediaFiles?.count ?? 0)")
        catalogDone = true
        if daemonMode {
            daemonCatalogReady.insert(ObjectIdentifier(device))
        }
        CFRunLoopStop(CFRunLoopGetMain())
    }


    func cameraDeviceDidRemoveAccessRestriction(_ device: ICDevice) {}

    func cameraDeviceDidEnableAccessRestriction(_ device: ICDevice) {}

    func cameraDevice(_ camera: ICCameraDevice, didReceiveThumbnail thumbnail: CGImage?, for item: ICCameraItem, error: (any Error)?) {
        if let thumbnail = thumbnail {
            thumbnailResults[item.name ?? ""] = thumbnail
        }
        pendingThumbnails -= 1
    }

    func cameraDevice(_ camera: ICCameraDevice, didCompleteDeleteFilesWithError error: (any Error)?) {
        deleteError = error
        deleteDone = true
        CFRunLoopStop(CFRunLoopGetMain())
    }

    // MARK: - ICCameraDeviceDownloadDelegate

    @objc func didDownloadFile(_ file: ICCameraFile, error: (any Error)?, options: [String: Any], contextInfo: UnsafeMutableRawPointer?) {
        downloadError = error

        if error == nil {
            if let savedFilename = options[ICDownloadOption.savedFilename.rawValue] as? String,
               let dirURL = options[ICDownloadOption.downloadsDirectoryURL.rawValue] as? URL {
                downloadedURL = dirURL.appendingPathComponent(savedFilename)
            }
        }

        downloadDone = true
        CFRunLoopStop(CFRunLoopGetMain())
    }

    /// Optional download-delegate hook: the only place ImageCaptureCore tells us
    /// anything mid-transfer. Declared with its explicit Objective-C selector
    /// because ImageCaptureCore discovers it via `respondsToSelector:`.
    ///
    /// Granularity is up to the device — some cameras report every few hundred
    /// KB, others once per file — so consumers must tolerate coarse updates.
    @objc(didReceiveDownloadProgressForFile:downloadedBytes:maxBytes:)
    func didReceiveDownloadProgress(for file: ICCameraFile, downloadedBytes: off_t, maxBytes: off_t) {
        guard let id = downloadProgressId else { return }
        downloadCurrentBytes = downloadedBytes

        let now = Date()
        guard now.timeIntervalSince(lastProgressEmit) >= downloadProgressInterval else { return }
        lastProgressEmit = now
        emitDownloadProgress(id: id)
    }

    /// Emit the batch-cumulative byte progress line for the in-flight download.
    func emitDownloadProgress(id: Int) {
        writeDaemonProgress(
            id: id,
            completed: downloadFilesCompleted,
            total: downloadFilesTotal,
            name: downloadCurrentName,
            bytesDone: downloadBytesBase + downloadCurrentBytes,
            bytesTotal: downloadBytesTotal
        )
    }

    // MARK: - One-shot Commands (CLI compat)

    func cmdScan() {
        let cameras = discoverCameras(keepBrowsing: false)
        let result: [[String: Any]] = cameras.map { camera in
            [
                "name": camera.name ?? "Unknown Camera",
                "serial": camera.serialNumberString ?? "",
                "model": camera.name ?? ""
            ]
        }
        printJSON(result)
    }

    func cmdCatalog(cameraName: String, thumbDir: String) {
        guard let camera = findCamera(name: cameraName) else {
            printError("Camera not found: \(cameraName)")
            return
        }

        // Set media presentation to original assets to get HIF+RAF pairs
        camera.mediaPresentation = .originalAssets

        guard openSession(camera: camera) else {
            printError("Failed to open session: \(sessionError?.localizedDescription ?? "timeout")")
            return
        }

        guard waitForCatalog(camera: camera) else {
            printError("Content cataloging timed out")
            _ = closeSession(camera: camera)
            stopBrowsing()
            return
        }

        // Get all media files
        let mediaFiles = (camera.mediaFiles ?? []).compactMap { $0 as? ICCameraFile }
        stderrLog("Found \(mediaFiles.count) media files")

        // Create thumb directory
        let thumbURL = URL(fileURLWithPath: thumbDir)
        try? FileManager.default.createDirectory(at: thumbURL, withIntermediateDirectories: true)

        // Request thumbnails for all files
        // RAF files share a stem with their rendered still and do not need a
        // duplicate thumbnail request. Avoiding them also cuts catalog IPC and
        // cache writes substantially on RAW+HEIF cards.
        let allItems = mediaFiles
            .filter { isPreviewableMedia($0.name ?? "") }
            .map { $0 as ICCameraItem }
        if !allItems.isEmpty {
            stderrLog("Requesting \(allItems.count) thumbnails...")
            requestAllThumbnails(camera: camera, items: allItems)
            stderrLog("Got \(thumbnailResults.count) thumbnails")
        }

        // Save thumbnails and build result
        var files: [[String: Any]] = []
        for file in mediaFiles {
            let name = file.name ?? ""
            let cacheId = thumbnailCacheId(name)
            var entry: [String: Any] = [
                "name": name,
                "size": file.fileSize,
                "uti": file.uti ?? "",
                "folder": file.parentFolder?.name ?? ""
            ]

            // Save thumbnail if we got one
            if let cgImage = thumbnailResults[name] {
                let thumbPath = thumbURL.appendingPathComponent("\(cacheId)_thumb.jpg")
                if saveCGImageAsJPEG(cgImage, to: thumbPath) {
                    entry["thumbnail"] = thumbPath.path
                }
            }

            files.append(entry)
        }

        _ = closeSession(camera: camera)
        stopBrowsing()

        let result: [String: Any] = [
            "camera": camera.name ?? "Unknown",
            "files": files
        ]
        printJSON(result)
    }

    func cmdDownload(cameraName: String, destDir: String, fileNames: [String]) {
        guard let camera = findCamera(name: cameraName) else {
            printError("Camera not found: \(cameraName)")
            return
        }

        camera.mediaPresentation = .originalAssets

        guard openSession(camera: camera) else {
            printError("Failed to open session: \(sessionError?.localizedDescription ?? "timeout")")
            return
        }

        guard waitForCatalog(camera: camera) else {
            printError("Content cataloging timed out")
            _ = closeSession(camera: camera)
            stopBrowsing()
            return
        }

        let destURL = URL(fileURLWithPath: destDir)
        try? FileManager.default.createDirectory(at: destURL, withIntermediateDirectories: true)

        let mediaFiles = (camera.mediaFiles ?? []).compactMap { $0 as? ICCameraFile }
        let fileNameSet = Set(fileNames)

        var downloaded: [[String: Any]] = []
        var errors: [String] = []

        for file in mediaFiles {
            guard let name = file.name, fileNameSet.contains(name) else { continue }

            stderrLog("Downloading \(name)...")
            if let resultURL = downloadFile(camera: camera, file: file, destDir: destURL) {
                downloaded.append([
                    "name": name,
                    "path": resultURL.path
                ])
            } else {
                errors.append("Failed to download \(name): \(downloadError?.localizedDescription ?? "unknown error")")
            }
        }

        _ = closeSession(camera: camera)
        stopBrowsing()

        let result: [String: Any] = [
            "downloaded": downloaded,
            "errors": errors
        ]
        printJSON(result)
    }

    func cmdDelete(cameraName: String, fileNames: [String]) {
        guard let camera = findCamera(name: cameraName) else {
            printError("Camera not found: \(cameraName)")
            return
        }

        camera.mediaPresentation = .originalAssets

        guard openSession(camera: camera) else {
            printError("Failed to open session: \(sessionError?.localizedDescription ?? "timeout")")
            return
        }

        guard waitForCatalog(camera: camera) else {
            printError("Content cataloging timed out")
            _ = closeSession(camera: camera)
            stopBrowsing()
            return
        }

        let mediaFiles = (camera.mediaFiles ?? []).compactMap { $0 as? ICCameraFile }
        let fileNameSet = Set(fileNames)
        let filesToDelete = mediaFiles.filter { fileNameSet.contains($0.name ?? "") }

        if filesToDelete.isEmpty {
            _ = closeSession(camera: camera)
            stopBrowsing()
            let result: [String: Any] = ["deleted": 0, "errors": ["No matching files found"]]
            printJSON(result)
            return
        }

        stderrLog("Deleting \(filesToDelete.count) files...")
        let error = deleteFiles(camera: camera, files: filesToDelete.map { $0 as ICCameraItem })
        _ = closeSession(camera: camera)
        stopBrowsing()

        var result: [String: Any] = ["deleted": filesToDelete.count]
        if let error = error {
            result["deleted"] = 0
            result["errors"] = [error]
        } else {
            result["errors"] = [String]()
        }
        printJSON(result)
    }

    // MARK: - Daemon Mode

    /// Look up a camera in the live daemon registry by serial, exact name, or
    /// case-insensitive name substring. Falls back to "the only camera" if
    /// exactly one is connected.
    func findDaemonCamera(identifier: String) -> ICCameraDevice? {
        // Exact serial match (serials are the primary key in daemonCameras)
        if let camera = daemonCameras[identifier] {
            return camera
        }
        // Exact name match
        for camera in daemonCameras.values {
            if camera.name == identifier { return camera }
        }
        // Case-insensitive substring match
        for camera in daemonCameras.values {
            if (camera.name ?? "").localizedCaseInsensitiveContains(identifier) {
                return camera
            }
        }
        // Single-camera fallback
        if daemonCameras.count == 1 {
            return daemonCameras.values.first
        }
        return nil
    }

    /// Write a single NDJSON response line to stdout. Bypasses Swift's stdio
    /// buffering so the Rust parent sees each response as soon as it's ready.
    func writeDaemonResponse(id: Int, ok: Bool, result: Any? = nil, error: String? = nil) {
        var response: [String: Any] = ["id": id, "ok": ok]
        if let result = result {
            response["result"] = result
        }
        if let error = error {
            response["error"] = error
        }

        let data: Data
        if let serialized = try? JSONSerialization.data(withJSONObject: response, options: [.sortedKeys]) {
            data = serialized
        } else {
            let fallback = "{\"id\":\(id),\"ok\":false,\"error\":\"response serialization failed\"}"
            data = fallback.data(using: .utf8) ?? Data()
        }

        FileHandle.standardOutput.write(data)
        FileHandle.standardOutput.write("\n".data(using: .utf8)!)
    }

    /// Write a non-terminal `progress` NDJSON line for an in-flight request.
    /// Unlike `writeDaemonResponse`, this carries no `ok`/`result` so the Rust
    /// reader routes it to the request's progress handler without completing the
    /// request. Same unbuffered write so the parent sees it immediately.
    /// `bytesDone` is cumulative across the whole batch (finished files plus the
    /// current file's partial transfer) so the Rust side can hand it straight to
    /// the UI's rate/ETA maths without tracking file boundaries itself.
    func writeDaemonProgress(id: Int, completed: Int, total: Int, name: String, bytesDone: Int64, bytesTotal: Int64) {
        let line: [String: Any] = [
            "id": id,
            "event": "progress",
            "completed": completed,
            "total": total,
            "name": name,
            "bytes_done": bytesDone,
            "bytes_total": bytesTotal
        ]

        let data: Data
        if let serialized = try? JSONSerialization.data(withJSONObject: line, options: [.sortedKeys]) {
            data = serialized
        } else {
            return
        }

        FileHandle.standardOutput.write(data)
        FileHandle.standardOutput.write("\n".data(using: .utf8)!)
    }

    /// Start the long-lived daemon. Called from the `daemon` CLI subcommand.
    /// Blocks the main thread forever on the run loop.
    func runDaemon() {
        daemonMode = true
        stderrLog("Daemon mode starting")
        browser.start()

        // Warm up: drain the run loop briefly so the initial burst of didAdd
        // callbacks lands before the first `scan` request. 3s is enough for
        // cameras already plugged in; new plugs after this point are handled
        // incrementally by the browser delegate.
        let warmDeadline = Date(timeIntervalSinceNow: 3.0)
        while Date() < warmDeadline {
            RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.1))
            if !daemonCameras.isEmpty { break }
        }
        stderrLog("Daemon warmed up with \(daemonCameras.count) camera(s)")

        // Sessions are kept open across requests, so something has to hand the
        // camera back when the user stops culling. The timer also guarantees the
        // run loop below always has a source, so it never returns immediately.
        daemonIdleTimer = Timer.scheduledTimer(withTimeInterval: daemonIdleCheckInterval, repeats: true) { [weak self] _ in
            self?.closeIdleSessions()
        }

        // Read stdin from a background thread. Each line is dispatched
        // synchronously to the main thread so ICCameraDevice operations
        // (which require main-thread delegate callbacks) work correctly.
        // Sync dispatch serializes commands naturally: the reader blocks
        // until each command returns before consuming the next line.
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            while let line = readLine() {
                if PtpBridge.isOutOfBandCommand(line) {
                    // `cancel` must not queue behind the download it cancels:
                    // sync dispatch would keep this thread — and therefore the
                    // next readLine() — blocked until that download finished.
                    // Async lands it in the main queue, which the download's
                    // nested run loop drains while it waits.
                    DispatchQueue.main.async {
                        self?.handleDaemonRequest(line)
                    }
                    continue
                }
                DispatchQueue.main.sync {
                    self?.handleDaemonRequest(line)
                }
            }
            // stdin closed → parent wants us to exit
            stderrLog("Daemon: stdin closed, exiting")
            DispatchQueue.main.sync {
                self?.closeAllDaemonSessions()
                self?.browser.stop()
                exit(0)
            }
        }

        // Main thread blocks here, draining device callbacks and dispatched
        // command handlers. Re-entered in a loop rather than using
        // `RunLoop.main.run()`: the delegate callbacks call CFRunLoopStop to
        // wake the nested waits, and a callback arriving while the daemon is
        // idle would otherwise return from the outermost run loop and exit.
        while true {
            RunLoop.main.run(until: Date(timeIntervalSinceNow: 1.0))
        }
    }

    /// Commands that may be handled re-entrantly while another command is still
    /// running on the main thread. Only cancellation qualifies: everything else
    /// touches session/download state that assumes serialized execution.
    static func isOutOfBandCommand(_ line: String) -> Bool {
        guard let data = line.data(using: .utf8),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let cmd = json["cmd"] as? String else {
            return false
        }
        return cmd == "cancel"
    }

    func handleDaemonRequest(_ line: String) {
        let trimmed = line.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }

        // Any activity keeps the held-open session alive, and the depth tells the
        // idle timer to stay out of the way while a command is running.
        daemonBusyDepth += 1
        daemonLastActivity = Date()
        defer {
            daemonBusyDepth -= 1
            daemonLastActivity = Date()
        }

        guard let data = trimmed.data(using: .utf8),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            writeDaemonResponse(id: 0, ok: false, error: "invalid JSON: \(trimmed)")
            return
        }

        let id = (json["id"] as? Int) ?? 0
        guard let cmd = json["cmd"] as? String else {
            writeDaemonResponse(id: id, ok: false, error: "missing 'cmd' field")
            return
        }

        switch cmd {
        case "scan":
            handleDaemonScan(id: id)

        case "catalog":
            guard let camera = json["camera"] as? String,
                  let thumbDir = json["thumb_dir"] as? String else {
                writeDaemonResponse(id: id, ok: false, error: "catalog requires 'camera' and 'thumb_dir'")
                return
            }
            handleDaemonCatalog(id: id, cameraName: camera, thumbDir: thumbDir)

        case "download":
            guard let camera = json["camera"] as? String,
                  let destDir = json["dest_dir"] as? String,
                  let files = json["files"] as? [String] else {
                writeDaemonResponse(id: id, ok: false, error: "download requires 'camera', 'dest_dir', 'files'")
                return
            }
            handleDaemonDownload(id: id, cameraName: camera, destDir: destDir, fileNames: files)

        case "delete":
            guard let camera = json["camera"] as? String,
                  let files = json["files"] as? [String] else {
                writeDaemonResponse(id: id, ok: false, error: "delete requires 'camera' and 'files'")
                return
            }
            handleDaemonDelete(id: id, cameraName: camera, fileNames: files)

        case "cancel":
            guard let camera = json["camera"] as? String else {
                writeDaemonResponse(id: id, ok: false, error: "cancel requires 'camera'")
                return
            }
            handleDaemonCancel(id: id, cameraName: camera)

        case "shutdown":
            writeDaemonResponse(id: id, ok: true, result: ["bye": true])
            closeAllDaemonSessions()
            browser.stop()
            exit(0)

        default:
            writeDaemonResponse(id: id, ok: false, error: "unknown command: \(cmd)")
        }
    }

    func handleDaemonScan(id: Int) {
        let result: [[String: Any]] = daemonCameras.values.map { camera in
            [
                "name": camera.name ?? "Unknown Camera",
                "serial": camera.serialNumberString ?? "",
                "model": camera.name ?? ""
            ]
        }
        writeDaemonResponse(id: id, ok: true, result: result)
    }

    func handleDaemonCatalog(id: Int, cameraName: String, thumbDir: String) {
        guard let camera = findDaemonCamera(identifier: cameraName) else {
            writeDaemonResponse(id: id, ok: false, error: "Camera not found: \(cameraName)")
            return
        }

        guard ensureSession(camera: camera) else {
            writeDaemonResponse(id: id, ok: false, error: "Failed to open session: \(sessionError?.localizedDescription ?? "timeout")")
            return
        }

        guard ensureCatalog(camera: camera) else {
            writeDaemonResponse(id: id, ok: false, error: "Content cataloging timed out")
            releaseSession(camera: camera)
            return
        }

        let mediaFiles = (camera.mediaFiles ?? []).compactMap { $0 as? ICCameraFile }
        stderrLog("Daemon: Found \(mediaFiles.count) media files")

        let thumbURL = URL(fileURLWithPath: thumbDir)
        try? FileManager.default.createDirectory(at: thumbURL, withIntermediateDirectories: true)

        let allItems = mediaFiles
            .filter { isPreviewableMedia($0.name ?? "") }
            .map { $0 as ICCameraItem }
        if !allItems.isEmpty {
            stderrLog("Daemon: Requesting \(allItems.count) thumbnails...")
            requestAllThumbnails(camera: camera, items: allItems)
            stderrLog("Daemon: Got \(thumbnailResults.count) thumbnails")
        }

        var files: [[String: Any]] = []
        for file in mediaFiles {
            let name = file.name ?? ""
            let cacheId = thumbnailCacheId(name)
            var entry: [String: Any] = [
                "name": name,
                "size": file.fileSize,
                "uti": file.uti ?? "",
                "folder": file.parentFolder?.name ?? ""
            ]

            if let cgImage = thumbnailResults[name] {
                let thumbPath = thumbURL.appendingPathComponent("\(cacheId)_thumb.jpg")
                if saveCGImageAsJPEG(cgImage, to: thumbPath) {
                    entry["thumbnail"] = thumbPath.path
                }
            }

            files.append(entry)
        }

        // NOTE: neither stopBrowsing() nor closeSession() here — the daemon keeps
        // the browser AND the camera session alive across requests, so the next
        // catalog/download doesn't have to rediscover the camera or re-wait for
        // its content catalog. The idle timer closes the session eventually.

        let result: [String: Any] = [
            "camera": camera.name ?? "Unknown",
            "files": files
        ]
        writeDaemonResponse(id: id, ok: true, result: result)
    }

    func handleDaemonDownload(id: Int, cameraName: String, destDir: String, fileNames: [String]) {
        guard let camera = findDaemonCamera(identifier: cameraName) else {
            writeDaemonResponse(id: id, ok: false, error: "Camera not found: \(cameraName)")
            return
        }

        guard ensureSession(camera: camera) else {
            writeDaemonResponse(id: id, ok: false, error: "Failed to open session: \(sessionError?.localizedDescription ?? "timeout")")
            return
        }

        guard ensureCatalog(camera: camera) else {
            writeDaemonResponse(id: id, ok: false, error: "Content cataloging timed out")
            releaseSession(camera: camera)
            return
        }

        let destURL = URL(fileURLWithPath: destDir)
        try? FileManager.default.createDirectory(at: destURL, withIntermediateDirectories: true)

        let fileNameSet = Set(fileNames)
        let mediaFiles = (camera.mediaFiles ?? []).compactMap { $0 as? ICCameraFile }
        let filesToDownload = mediaFiles.filter { ($0.name).map(fileNameSet.contains) ?? false }
        let totalToDownload = filesToDownload.count

        // Byte totals come from the catalog's fileSize rather than the download
        // delegate's maxBytes, so the UI has a stable denominator for the whole
        // batch from the very first progress line.
        downloadProgressId = id
        downloadBytesBase = 0
        downloadBytesTotal = filesToDownload.reduce(Int64(0)) { $0 + $1.fileSize }
        downloadCurrentBytes = 0
        downloadFilesCompleted = 0
        downloadFilesTotal = totalToDownload
        downloadCurrentName = ""
        lastProgressEmit = Date.distantPast
        downloadCancelled = false
        defer {
            downloadProgressId = nil
            downloadCurrentBytes = 0
        }

        var downloaded: [[String: Any]] = []
        var errors: [String] = []

        for file in filesToDownload {
            guard let name = file.name else { continue }

            if downloadCancelled {
                errors.append("Download cancelled before \(name)")
                continue
            }

            downloadCurrentName = name
            downloadCurrentBytes = 0
            stderrLog("Daemon: Downloading \(name) (\(file.fileSize) bytes)...")
            if let resultURL = downloadFile(camera: camera, file: file, destDir: destURL) {
                downloaded.append([
                    "name": name,
                    "path": resultURL.path
                ])
            } else if downloadCancelled {
                errors.append("Download of \(name) cancelled")
            } else {
                errors.append("Failed to download \(name): \(downloadError?.localizedDescription ?? "unknown error")")
            }

            // The file settled (success, failure or cancellation): fold its full
            // size into the base so cumulative bytes stay monotonic even when the
            // device reported progress coarsely, and emit unthrottled so the
            // counter never lags behind a completed file.
            downloadFilesCompleted = downloaded.count
            downloadBytesBase += file.fileSize
            downloadCurrentBytes = 0
            lastProgressEmit = Date()
            emitDownloadProgress(id: id)
        }

        // Session intentionally left open — see handleDaemonCatalog.

        let result: [String: Any] = [
            "downloaded": downloaded,
            "errors": errors
        ]
        writeDaemonResponse(id: id, ok: true, result: result)
    }

    /// Cancel whatever this camera is currently downloading. Dispatched
    /// out-of-band (see `isOutOfBandCommand`), so this runs re-entrantly inside
    /// the run loop the download itself is spinning; `downloadCancelled` is what
    /// tells that loop to stop after the current file unwinds.
    func handleDaemonCancel(id: Int, cameraName: String) {
        guard let camera = findDaemonCamera(identifier: cameraName) else {
            writeDaemonResponse(id: id, ok: false, error: "Camera not found: \(cameraName)")
            return
        }

        downloadCancelled = true
        camera.cancelDownload()
        stderrLog("Daemon: cancel requested on '\(camera.name ?? "?")'")
        writeDaemonResponse(id: id, ok: true, result: ["cancelled": true])
    }

    func handleDaemonDelete(id: Int, cameraName: String, fileNames: [String]) {
        guard let camera = findDaemonCamera(identifier: cameraName) else {
            writeDaemonResponse(id: id, ok: false, error: "Camera not found: \(cameraName)")
            return
        }

        // Deletes run on the same held-open session as the import that preceded
        // them, which removes the race `closeSession` documents (a delete opening
        // a session while the download session's async close was still in flight)
        // instead of merely sequencing around it. If a body ever turns out to
        // refuse deletes on a long-lived session, bracket just this handler in
        // releaseSession + ensureSession.
        guard ensureSession(camera: camera) else {
            writeDaemonResponse(id: id, ok: false, error: "Failed to open session: \(sessionError?.localizedDescription ?? "timeout")")
            return
        }

        guard ensureCatalog(camera: camera) else {
            writeDaemonResponse(id: id, ok: false, error: "Content cataloging timed out")
            releaseSession(camera: camera)
            return
        }

        let mediaFiles = (camera.mediaFiles ?? []).compactMap { $0 as? ICCameraFile }
        let fileNameSet = Set(fileNames)
        let filesToDelete = mediaFiles.filter { fileNameSet.contains($0.name ?? "") }

        if filesToDelete.isEmpty {
            let result: [String: Any] = ["deleted": 0, "errors": ["No matching files found"]]
            writeDaemonResponse(id: id, ok: true, result: result)
            return
        }

        stderrLog("Daemon: Deleting \(filesToDelete.count) files...")
        let error = deleteFiles(camera: camera, files: filesToDelete.map { $0 as ICCameraItem })

        var result: [String: Any] = ["deleted": filesToDelete.count]
        if let error = error {
            result["deleted"] = 0
            result["errors"] = [error]
        } else {
            result["errors"] = [String]()
        }
        writeDaemonResponse(id: id, ok: true, result: result)
    }
}

// MARK: - Main

let bridge = PtpBridge()
let args = Array(CommandLine.arguments.dropFirst())

guard let command = args.first else {
    printError("Usage: ptp-bridge <daemon|scan|catalog|download|delete> [args...]")
    exit(1)
}

switch command {
case "daemon":
    // Blocks forever; only returns via exit() on stdin close or shutdown cmd.
    bridge.runDaemon()

case "scan":
    bridge.cmdScan()

case "catalog":
    guard args.count >= 3 else {
        printError("Usage: ptp-bridge catalog <camera-name> <thumb-cache-dir>")
        exit(1)
    }
    bridge.cmdCatalog(cameraName: args[1], thumbDir: args[2])

case "download":
    guard args.count >= 4 else {
        printError("Usage: ptp-bridge download <camera-name> <dest-dir> <file1> [file2...]")
        exit(1)
    }
    bridge.cmdDownload(cameraName: args[1], destDir: args[2], fileNames: Array(args[3...]))

case "delete":
    guard args.count >= 3 else {
        printError("Usage: ptp-bridge delete <camera-name> <file1> [file2...]")
        exit(1)
    }
    bridge.cmdDelete(cameraName: args[1], fileNames: Array(args[2...]))

default:
    printError("Unknown command: \(command). Available: daemon, scan, catalog, download, delete")
    exit(1)
}

exit(0)
