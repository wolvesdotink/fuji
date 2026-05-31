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

// MARK: - PTP Bridge

class PtpBridge: NSObject, ICDeviceBrowserDelegate, ICCameraDeviceDelegate, ICCameraDeviceDownloadDelegate {
    let browser = ICDeviceBrowser()
    var discoveredCameras: [ICCameraDevice] = []

    // Session state
    var sessionOpened = false
    var sessionError: Error?

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

    func openSession(camera: ICCameraDevice, timeout: TimeInterval = 10.0) -> Bool {
        sessionOpened = false
        sessionError = nil
        catalogDone = false
        camera.delegate = self
        camera.requestOpenSession()

        let deadline = Date(timeIntervalSinceNow: timeout)
        while !sessionOpened && Date() < deadline {
            RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.1))
        }

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

    // MARK: - Thumbnail Requests

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

        let deadline = Date(timeIntervalSinceNow: timeout)
        while !downloadDone && Date() < deadline {
            RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.1))
        }

        if let error = downloadError {
            stderrLog("Download error: \(error.localizedDescription)")
            return nil
        }

        return downloadedURL
    }

    // MARK: - File Delete

    func deleteFiles(camera: ICCameraDevice, files: [ICCameraItem], timeout: TimeInterval = 60.0) -> Error? {
        deleteDone = false
        deleteError = nil

        camera.requestDeleteFiles(files)

        let deadline = Date(timeIntervalSinceNow: timeout)
        while !deleteDone && Date() < deadline {
            RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.1))
        }

        return deleteError
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
    }

    func device(_ device: ICDevice, didCloseSessionWithError error: (any Error)?) {}

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
            camera.requestCloseSession()
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
        let allItems = mediaFiles.map { $0 as ICCameraItem }
        if !allItems.isEmpty {
            stderrLog("Requesting \(allItems.count) thumbnails...")
            requestAllThumbnails(camera: camera, items: allItems)
            stderrLog("Got \(thumbnailResults.count) thumbnails")
        }

        // Save thumbnails and build result
        var files: [[String: Any]] = []
        for file in mediaFiles {
            let name = file.name ?? ""
            let stem = (name as NSString).deletingPathExtension
            var entry: [String: Any] = [
                "name": name,
                "size": file.fileSize,
                "uti": file.uti ?? "",
                "folder": file.parentFolder?.name ?? ""
            ]

            // Save thumbnail if we got one
            if let cgImage = thumbnailResults[name] {
                let thumbPath = thumbURL.appendingPathComponent("\(stem)_thumb.jpg")
                if saveCGImageAsJPEG(cgImage, to: thumbPath) {
                    entry["thumbnail"] = thumbPath.path
                }
            }

            files.append(entry)
        }

        camera.requestCloseSession()
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
            camera.requestCloseSession()
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

        camera.requestCloseSession()
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
            camera.requestCloseSession()
            stopBrowsing()
            return
        }

        let mediaFiles = (camera.mediaFiles ?? []).compactMap { $0 as? ICCameraFile }
        let fileNameSet = Set(fileNames)
        let filesToDelete = mediaFiles.filter { fileNameSet.contains($0.name ?? "") }

        if filesToDelete.isEmpty {
            camera.requestCloseSession()
            stopBrowsing()
            let result: [String: Any] = ["deleted": 0, "errors": ["No matching files found"]]
            printJSON(result)
            return
        }

        stderrLog("Deleting \(filesToDelete.count) files...")
        let error = deleteFiles(camera: camera, files: filesToDelete.map { $0 as ICCameraItem })
        camera.requestCloseSession()
        stopBrowsing()

        var result: [String: Any] = ["deleted": filesToDelete.count]
        if let error = error {
            result["errors"] = [error.localizedDescription]
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
    func writeDaemonProgress(id: Int, completed: Int, total: Int, name: String) {
        let line: [String: Any] = [
            "id": id,
            "event": "progress",
            "completed": completed,
            "total": total,
            "name": name
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

        // Read stdin from a background thread. Each line is dispatched
        // synchronously to the main thread so ICCameraDevice operations
        // (which require main-thread delegate callbacks) work correctly.
        // Sync dispatch serializes commands naturally: the reader blocks
        // until each command returns before consuming the next line.
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            while let line = readLine() {
                DispatchQueue.main.sync {
                    self?.handleDaemonRequest(line)
                }
            }
            // stdin closed → parent wants us to exit
            stderrLog("Daemon: stdin closed, exiting")
            DispatchQueue.main.sync {
                self?.browser.stop()
                exit(0)
            }
        }

        // Main thread blocks here forever, draining device callbacks and
        // dispatched command handlers.
        RunLoop.main.run()
    }

    func handleDaemonRequest(_ line: String) {
        let trimmed = line.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }

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

        case "shutdown":
            writeDaemonResponse(id: id, ok: true, result: ["bye": true])
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

        camera.mediaPresentation = .originalAssets

        guard openSession(camera: camera) else {
            writeDaemonResponse(id: id, ok: false, error: "Failed to open session: \(sessionError?.localizedDescription ?? "timeout")")
            return
        }

        guard waitForCatalog(camera: camera) else {
            writeDaemonResponse(id: id, ok: false, error: "Content cataloging timed out")
            camera.requestCloseSession()
            return
        }

        let mediaFiles = (camera.mediaFiles ?? []).compactMap { $0 as? ICCameraFile }
        stderrLog("Daemon: Found \(mediaFiles.count) media files")

        let thumbURL = URL(fileURLWithPath: thumbDir)
        try? FileManager.default.createDirectory(at: thumbURL, withIntermediateDirectories: true)

        let allItems = mediaFiles.map { $0 as ICCameraItem }
        if !allItems.isEmpty {
            stderrLog("Daemon: Requesting \(allItems.count) thumbnails...")
            requestAllThumbnails(camera: camera, items: allItems)
            stderrLog("Daemon: Got \(thumbnailResults.count) thumbnails")
        }

        var files: [[String: Any]] = []
        for file in mediaFiles {
            let name = file.name ?? ""
            let stem = (name as NSString).deletingPathExtension
            var entry: [String: Any] = [
                "name": name,
                "size": file.fileSize,
                "uti": file.uti ?? "",
                "folder": file.parentFolder?.name ?? ""
            ]

            if let cgImage = thumbnailResults[name] {
                let thumbPath = thumbURL.appendingPathComponent("\(stem)_thumb.jpg")
                if saveCGImageAsJPEG(cgImage, to: thumbPath) {
                    entry["thumbnail"] = thumbPath.path
                }
            }

            files.append(entry)
        }

        camera.requestCloseSession()
        // NOTE: do NOT call stopBrowsing() — the daemon keeps the browser alive
        // across requests so the next catalog/download doesn't have to
        // rediscover the camera.

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

        camera.mediaPresentation = .originalAssets

        guard openSession(camera: camera) else {
            writeDaemonResponse(id: id, ok: false, error: "Failed to open session: \(sessionError?.localizedDescription ?? "timeout")")
            return
        }

        guard waitForCatalog(camera: camera) else {
            writeDaemonResponse(id: id, ok: false, error: "Content cataloging timed out")
            camera.requestCloseSession()
            return
        }

        let destURL = URL(fileURLWithPath: destDir)
        try? FileManager.default.createDirectory(at: destURL, withIntermediateDirectories: true)

        let fileNameSet = Set(fileNames)
        let mediaFiles = (camera.mediaFiles ?? []).compactMap { $0 as? ICCameraFile }
        let filesToDownload = mediaFiles.filter { ($0.name).map(fileNameSet.contains) ?? false }
        let totalToDownload = filesToDownload.count

        var downloaded: [[String: Any]] = []
        var errors: [String] = []

        for file in filesToDownload {
            guard let name = file.name else { continue }

            stderrLog("Daemon: Downloading \(name)...")
            if let resultURL = downloadFile(camera: camera, file: file, destDir: destURL) {
                downloaded.append([
                    "name": name,
                    "path": resultURL.path
                ])
            } else {
                errors.append("Failed to download \(name): \(downloadError?.localizedDescription ?? "unknown error")")
            }

            // Emit live progress after each file settles (success or failure) so
            // the import counter reflects the actual number imported so far.
            writeDaemonProgress(
                id: id,
                completed: downloaded.count,
                total: totalToDownload,
                name: name
            )
        }

        camera.requestCloseSession()

        let result: [String: Any] = [
            "downloaded": downloaded,
            "errors": errors
        ]
        writeDaemonResponse(id: id, ok: true, result: result)
    }

    func handleDaemonDelete(id: Int, cameraName: String, fileNames: [String]) {
        guard let camera = findDaemonCamera(identifier: cameraName) else {
            writeDaemonResponse(id: id, ok: false, error: "Camera not found: \(cameraName)")
            return
        }

        camera.mediaPresentation = .originalAssets

        guard openSession(camera: camera) else {
            writeDaemonResponse(id: id, ok: false, error: "Failed to open session: \(sessionError?.localizedDescription ?? "timeout")")
            return
        }

        guard waitForCatalog(camera: camera) else {
            writeDaemonResponse(id: id, ok: false, error: "Content cataloging timed out")
            camera.requestCloseSession()
            return
        }

        let mediaFiles = (camera.mediaFiles ?? []).compactMap { $0 as? ICCameraFile }
        let fileNameSet = Set(fileNames)
        let filesToDelete = mediaFiles.filter { fileNameSet.contains($0.name ?? "") }

        if filesToDelete.isEmpty {
            camera.requestCloseSession()
            let result: [String: Any] = ["deleted": 0, "errors": ["No matching files found"]]
            writeDaemonResponse(id: id, ok: true, result: result)
            return
        }

        stderrLog("Daemon: Deleting \(filesToDelete.count) files...")
        let error = deleteFiles(camera: camera, files: filesToDelete.map { $0 as ICCameraItem })
        camera.requestCloseSession()

        var result: [String: Any] = ["deleted": filesToDelete.count]
        if let error = error {
            result["errors"] = [error.localizedDescription]
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
