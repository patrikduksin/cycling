import CoreBluetooth
import Foundation

final class VanaLink: ExactModule {
    private lazy var link = Link(context: context)
    override func call(_ request: [String: Any]) throws -> [String: Any] { link.status() }
    override func later(_ request: [String: Any], reply: ExactReply) { link.later(request, reply: reply) }
    override func destroy() { link.close() }
}

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

    /// HTTP over the channel, one request at a time: the rest wait in `queue`.
    /// `to` is a file under the app's cache that takes a 2xx reply's body as bytes.
    private struct Exchange { let request: Data; let reply: ExactReply; let timeout: Double; let to: URL? }
    private var queue: [Exchange] = []
    private var current: Exchange?, currentTimeout: DispatchWorkItem?
    private var outbox: [UInt8] = [], inbox: [UInt8] = [], response: [UInt8] = []

    func status() -> [String: Any] {
        ["state": state, "detail": detail, "name": name, "psm": psm]
    }

    func later(_ request: [String: Any], reply: ExactReply) {
        DispatchQueue.main.async { [self] in
            switch request["op"] as? String {
            case "status": reply.send(status())
            case "connect": connect(reply)
            case "http": http(request, reply)
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

    private func http(_ request: [String: Any], _ reply: ExactReply) {
        guard channel != nil else { return reply.fail("not connected") }
        let method = request["method"] as? String ?? "GET", path = request["path"] as? String ?? "/"
        let body = Data((request["body"] as? String ?? "").utf8)
        var bytes = Data("\(method) \(path) HTTP/1.1\r\nHost: vana\r\nContent-Length: \(body.count)\r\n\r\n".utf8)
        bytes.append(body)
        let timeout = (request["timeout_ms"] as? Double ?? 15_000) / 1000
        let to = (request["to"] as? String).map { context.cache.appendingPathComponent($0) }
        let exchange = Exchange(request: bytes, reply: reply, timeout: timeout, to: to)
        let after = (request["after_ms"] as? Double ?? 0) / 1000
        DispatchQueue.main.asyncAfter(deadline: .now() + after) { [self] in
            guard channel != nil else { return reply.fail("not connected") }
            queue.append(exchange)
            nextExchange()
        }
    }

    private func nextExchange() {
        guard current == nil, !queue.isEmpty else { return }
        let exchange = queue.removeFirst()
        current = exchange
        outbox = Array(exchange.request); inbox = []; response = []
        // A late reply would be read as the next one's, so a timeout ends the channel.
        let timeout = DispatchWorkItem { [weak self] in self?.channelClosed("request timed out") }
        currentTimeout = timeout
        DispatchQueue.main.asyncAfter(deadline: .now() + exchange.timeout, execute: timeout)
        flush()
    }

    private func flush() {
        guard let out = channel?.outputStream else { return }
        while !outbox.isEmpty, out.hasSpaceAvailable {
            let n = outbox.withUnsafeBufferPointer { out.write($0.baseAddress!, maxLength: $0.count) }
            if n <= 0 { break }
            outbox.removeFirst(n)
        }
    }

    /// Frames are a u16 LE length and that many bytes of the response; a zero length ends it.
    private func receive() {
        guard let input = channel?.inputStream else { return }
        var buffer = [UInt8](repeating: 0, count: 4096)
        while input.hasBytesAvailable {
            let n = input.read(&buffer, maxLength: buffer.count)
            if n <= 0 { break }
            inbox.append(contentsOf: buffer[0..<n])
        }
        guard current != nil else { inbox = []; return }
        while inbox.count >= 2 {
            let length = Int(inbox[0]) | Int(inbox[1]) << 8
            if length == 0 {
                inbox.removeFirst(2)
                return finishExchange()
            }
            guard inbox.count >= 2 + length else { return }
            response.append(contentsOf: inbox[2..<(2 + length)])
            inbox.removeFirst(2 + length)
        }
    }

    private func finishExchange() {
        currentTimeout?.cancel(); currentTimeout = nil
        let exchange = current; current = nil
        if let to = exchange?.to, let split = headEnd(response), response.starts(with: Array("HTTP/1.1 2".utf8)) {
            do {
                try FileManager.default.createDirectory(at: to.deletingLastPathComponent(), withIntermediateDirectories: true)
                try Data(response[split...]).write(to: to, options: .atomic)
                exchange?.reply.send(["response": String(decoding: response[..<split], as: UTF8.self), "saved": response.count - split])
            } catch {
                exchange?.reply.fail("could not save the file: \(error.localizedDescription)")
            }
        } else {
            exchange?.reply.send(["response": String(decoding: response, as: UTF8.self)])
        }
        response = []
        nextExchange()
    }

    private func headEnd(_ bytes: [UInt8]) -> Int? {
        let gap: [UInt8] = [13, 10, 13, 10]
        guard bytes.count >= 4 else { return nil }
        return (0...(bytes.count - 4)).first { Array(bytes[$0..<($0 + 4)]) == gap }.map { $0 + 4 }
    }

    private func failExchanges(_ why: String) {
        currentTimeout?.cancel(); currentTimeout = nil
        current?.reply.fail(why); current = nil
        for exchange in queue { exchange.reply.fail(why) }
        queue = []; outbox = []; inbox = []; response = []
    }

    private func channelClosed(_ why: String) {
        closeChannel()
        set("failed", why)
    }

    private func closeChannel() {
        failExchanges("channel closed")
        if let ch = channel {
            for s in [ch.inputStream as Stream, ch.outputStream as Stream] { s.close(); s.remove(from: .main, forMode: .default) }
        }
        channel = nil
    }

    func stream(_ s: Stream, handle event: Stream.Event) {
        switch event {
        case .hasBytesAvailable: receive()
        case .hasSpaceAvailable: flush()
        case .errorOccurred, .endEncountered: channelClosed("channel closed")
        default: break
        }
    }
}
let exactModule: ExactModule.Type = VanaLink.self
