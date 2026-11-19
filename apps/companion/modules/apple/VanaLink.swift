import CoreBluetooth
import Foundation

/// The module Exact loads: it owns the link, which CoreBluetooth needs to be an NSObject.
final class VanaLink: ExactModule {
    private lazy var link = Link(context: context)
    override func call(_ request: [String: Any]) throws -> [String: Any] { link.status() }
    override func later(_ request: [String: Any], reply: ExactReply) { link.later(request, reply: reply) }
    override func destroy() { link.close() }
}

/// VANA's link over Bluetooth LE: find the sync service, read the L2CAP PSM it
/// publishes, open the channel. SPIKE: an echo test measures the channel.
final class Link: NSObject, CBCentralManagerDelegate, CBPeripheralDelegate, StreamDelegate {
    private let context: ExactModuleContext
    init(context: ExactModuleContext) { self.context = context }

    static let service = CBUUID(string: "56414E41-0001-4C69-6E6B-C606C606C606")
    static let psmCharacteristic = CBUUID(string: "56414E41-0002-4C69-6E6B-C606C606C606")

    private var central: CBCentralManager?
    private var peripheral: CBPeripheral?
    private var channel: CBL2CAPChannel?
    private var state = "idle", detail = "", name = "", psm = 0
    private var connecting: ExactReply?, connectStart = Date()
    private var connectTimeout: DispatchWorkItem?

    // Echo test state.
    private var echo: ExactReply?, echoStart = Date()
    private var outgoing = Data(), sent = 0, received = 0, mismatches = 0
    /// Bytes expected back: the echo's own size, or a download's.
    private var expected = 0, oneWay = false, shown = 0

    func status() -> [String: Any] {
        ["state": state, "detail": detail, "name": name, "psm": psm]
    }

    func later(_ request: [String: Any], reply: ExactReply) {
        DispatchQueue.main.async { [self] in
            switch request["op"] as? String {
            case "connect": connect(reply)
            case "echo": startEcho(bytes: (request["bytes"] as? NSNumber)?.intValue ?? 102_400, reply)
            case "download": startDownload(bytes: (request["bytes"] as? NSNumber)?.intValue ?? 512_000, reply)
            case "disconnect":
                closeChannel()
                if let p = peripheral { central?.cancelPeripheralConnection(p) }
                set("idle", "")
                reply.send(["ok": true])
            default: reply.fail("unknown op")
            }
        }
    }

    func close() { closeChannel() }

    private func set(_ s: String, _ d: String) {
        state = s; detail = d
        context.changed("vana.link")
    }

    // MARK: connect

    private func connect(_ reply: ExactReply) {
        connecting?.fail("superseded")
        connecting = reply; connectStart = Date()
        closeChannel()
        let timeout = DispatchWorkItem { [weak self] in self?.failConnect("timed out") }
        connectTimeout?.cancel(); connectTimeout = timeout
        DispatchQueue.main.asyncAfter(deadline: .now() + 20, execute: timeout)
        if central == nil {
            central = CBCentralManager(delegate: self, queue: .main)
        } else {
            scan()
        }
    }

    private func scan() {
        guard let central, central.state == .poweredOn else { return }
        if let known = central.retrieveConnectedPeripherals(withServices: [Self.service]).first {
            use(known)
            return
        }
        set("scanning", "")
        central.scanForPeripherals(withServices: [Self.service])
    }

    private func use(_ p: CBPeripheral) {
        central?.stopScan()
        peripheral = p; p.delegate = self
        name = p.name ?? "VANA"
        set("connecting", name)
        central?.connect(p)
    }

    private func failConnect(_ why: String) {
        connectTimeout?.cancel()
        central?.stopScan()
        set("failed", why)
        connecting?.fail(why); connecting = nil
    }

    func centralManagerDidUpdateState(_ central: CBCentralManager) {
        switch central.state {
        case .poweredOn: if connecting != nil { scan() }
        case .unauthorized: failConnect("Bluetooth permission denied")
        case .poweredOff: failConnect("Bluetooth is off")
        case .unsupported: failConnect("Bluetooth LE unsupported")
        default: break
        }
    }

    func centralManager(_ central: CBCentralManager, didDiscover p: CBPeripheral, advertisementData: [String: Any], rssi: NSNumber) {
        use(p)
    }

    func centralManager(_ central: CBCentralManager, didConnect p: CBPeripheral) {
        set("discovering", name)
        p.discoverServices([Self.service])
    }

    func centralManager(_ central: CBCentralManager, didFailToConnect p: CBPeripheral, error: Error?) {
        failConnect("connect failed: \(error?.localizedDescription ?? "unknown")")
    }

    func centralManager(_ central: CBCentralManager, didDisconnectPeripheral p: CBPeripheral, error: Error?) {
        closeChannel()
        if connecting != nil { failConnect("disconnected") } else { set("idle", "disconnected") }
        echo?.fail("disconnected"); echo = nil
    }

    func peripheral(_ p: CBPeripheral, didDiscoverServices error: Error?) {
        guard let s = p.services?.first(where: { $0.uuid == Self.service }) else { return failConnect("no sync service") }
        p.discoverCharacteristics([Self.psmCharacteristic], for: s)
    }

    func peripheral(_ p: CBPeripheral, didDiscoverCharacteristicsFor s: CBService, error: Error?) {
        guard let c = s.characteristics?.first(where: { $0.uuid == Self.psmCharacteristic }) else { return failConnect("no PSM characteristic") }
        p.readValue(for: c)
    }

    func peripheral(_ p: CBPeripheral, didUpdateValueFor c: CBCharacteristic, error: Error?) {
        guard let v = c.value, v.count >= 2 else { return failConnect("bad PSM") }
        psm = Int(v[0]) | Int(v[1]) << 8
        set("opening", "PSM \(psm)")
        p.openL2CAPChannel(CBL2CAPPSM(psm))
    }

    func peripheral(_ p: CBPeripheral, didOpen ch: CBL2CAPChannel?, error: Error?) {
        guard let ch, error == nil else { return failConnect("L2CAP open failed: \(error?.localizedDescription ?? "nil")") }
        channel = ch
        for s in [ch.inputStream as Stream, ch.outputStream as Stream] {
            s.delegate = self
            s.schedule(in: .main, forMode: .default)
            s.open()
        }
        connectTimeout?.cancel()
        let ms = Int(Date().timeIntervalSince(connectStart) * 1000)
        set("open", "\(name), PSM \(psm), \(ms) ms")
        connecting?.send(["ok": true, "name": name, "psm": psm, "ms": ms]); connecting = nil
    }

    private func closeChannel() {
        if let ch = channel {
            for s in [ch.inputStream as Stream, ch.outputStream as Stream] { s.close(); s.remove(from: .main, forMode: .default) }
        }
        channel = nil
    }

    // MARK: echo (SPIKE)

    private func pattern(_ i: Int) -> UInt8 { UInt8(truncatingIfNeeded: i &* 31 &+ 7) }

    private func startEcho(bytes: Int, _ reply: ExactReply) {
        guard channel != nil else { return reply.fail("not connected") }
        echo?.fail("superseded")
        echo = reply; echoStart = Date()
        outgoing = Data((0..<bytes).map(pattern)); sent = 0; received = 0; mismatches = 0; shown = 0
        expected = bytes; oneWay = false
        set("echo", "0 / \(bytes)")
        pump()
    }

    /// SPIKE: VANA streams `bytes` of the pattern; measures the one-way rate a FIT download gets.
    private func startDownload(bytes: Int, _ reply: ExactReply) {
        guard channel != nil else { return reply.fail("not connected") }
        echo?.fail("superseded")
        echo = reply; echoStart = Date()
        var ask = Data("DL".utf8)
        withUnsafeBytes(of: UInt32(bytes).littleEndian) { ask.append(contentsOf: $0) }
        outgoing = ask; sent = 0; received = 0; mismatches = 0; shown = 0
        expected = bytes; oneWay = true
        set("download", "0 / \(bytes)")
        pump()
    }

    private func pump() {
        guard let out = channel?.outputStream, echo != nil else { return }
        while sent < outgoing.count && out.hasSpaceAvailable {
            let n = outgoing[sent..<min(sent + 512, outgoing.count)].withUnsafeBytes {
                out.write($0.bindMemory(to: UInt8.self).baseAddress!, maxLength: $0.count)
            }
            if n <= 0 { break }
            sent += n
        }
    }

    private func drain() {
        guard let input = channel?.inputStream else { return }
        var buf = [UInt8](repeating: 0, count: 4096)
        while input.hasBytesAvailable {
            let n = input.read(&buf, maxLength: buf.count)
            if n <= 0 { break }
            for i in 0..<n where buf[i] != pattern(received + i) { mismatches += 1 }
            received += n
        }
        guard let reply = echo else { return }
        if received >= expected {
            let ms = max(1, Int(Date().timeIntervalSince(echoStart) * 1000))
            let kbps = Double(received) * (oneWay ? 1 : 2) / Double(ms) // KB/s, both directions for an echo
            set("open", "\(oneWay ? "download" : "echo") \(received) B in \(ms) ms")
            reply.send(["ok": mismatches == 0, "bytes": received, "ms": ms, "kbps": kbps, "mismatches": mismatches])
            echo = nil
        } else if received - shown >= 8192 {
            shown = received
            set(oneWay ? "download" : "echo", "\(received) / \(expected)")
        }
    }

    func stream(_ s: Stream, handle event: Stream.Event) {
        switch event {
        case .hasSpaceAvailable: pump()
        case .hasBytesAvailable: drain()
        case .errorOccurred, .endEncountered:
            closeChannel()
            set("failed", "channel closed")
            echo?.fail("channel closed"); echo = nil
        default: break
        }
    }
}
let exactModule: ExactModule.Type = VanaLink.self
