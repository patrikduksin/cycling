import CoreBluetooth
import Foundation
import Security

final class VanaLink: ExactModule {
    private lazy var link = Link(context: context)
    override func call(_ request: [String: Any]) throws -> [String: Any] { link.status() }
    override func later(_ request: [String: Any], reply: ExactReply) { link.later(request, reply: reply) }
    override func destroy() { link.close() }
}

/// A pipe to the paired bike computer: it finds the device whose advertisement
/// carries the paired id, opens the L2CAP channel and moves frames. The data
/// module owns the handshake, the encryption and HTTP.
final class Link: NSObject, CBCentralManagerDelegate, CBPeripheralDelegate, StreamDelegate {
    private let context: ExactModuleContext
    init(context: ExactModuleContext) { self.context = context }

    static let service = CBUUID(string: "56414E41-0001-4C69-6E6B-C606C606C606")
    static let psmCharacteristic = CBUUID(string: "56414E41-0002-4C69-6E6B-C606C606C606")

    private var central: CBCentralManager?
    private var peripheral: CBPeripheral?
    private var channel: CBL2CAPChannel?
    private var state = "idle", detail = "", name = "", psm = 0
    private var wanted = Data()
    private var connecting: ExactReply?, connectStart = Date()
    private var connectTimeout: DispatchWorkItem?

    /// One write and the frames that answer it: one frame (`frame`), or up to
    /// the 16-byte frame of an empty plaintext (`end`). Exchanges run one at a
    /// time in the order they came, each no sooner than `due`.
    private struct Exchange { let send: Data; let untilEnd: Bool; let due: Date; let timeout: Double; let reply: ExactReply }
    private var queue: [Exchange] = []
    private var current: Exchange?, currentTimeout: DispatchWorkItem?
    private var outbox: [UInt8] = [], inbox: [UInt8] = [], collected: [UInt8] = []
    /// Frames the device sent since the channel opened; `at` is the first of the current reply's.
    private var received = 0, at = 0

    func status() -> [String: Any] {
        ["state": state, "detail": detail, "name": name, "psm": psm]
    }

    func later(_ request: [String: Any], reply: ExactReply) {
        DispatchQueue.main.async { [self] in
            switch request["op"] as? String {
            case "status": reply.send(status())
            case "connect": connect(request, reply)
            case "exchange": exchange(request, reply)
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

    private func connect(_ request: [String: Any], _ reply: ExactReply) {
        guard let id = (request["id"] as? String).flatMap(hexBytes), id.count == 4 else { return reply.fail("no paired id") }
        connecting?.fail("superseded")
        connecting = reply; connectStart = Date(); wanted = id
        closeChannel()
        let timeout = DispatchWorkItem { [weak self] in self?.failConnect("Bike computer not found. Is it on and not recording?") }
        connectTimeout?.cancel(); connectTimeout = timeout
        DispatchQueue.main.asyncAfter(deadline: .now() + 20, execute: timeout)
        if central == nil {
            central = CBCentralManager(delegate: self, queue: .main)
        } else {
            scan()
        }
    }

    private func hexBytes(_ hex: String) -> Data? {
        var out = Data()
        var i = hex.startIndex
        while i < hex.endIndex {
            guard let j = hex.index(i, offsetBy: 2, limitedBy: hex.endIndex), let b = UInt8(hex[i..<j], radix: 16) else { return nil }
            out.append(b); i = j
        }
        return out
    }

    private func scan() {
        guard let central, central.state == .poweredOn else { return }
        if let known = central.retrieveConnectedPeripherals(withServices: [Self.service]).first {
            use(known)
            return
        }
        set("scanning", "")
        central.scanForPeripherals(withServices: nil)
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

    /// The paired bike computer puts its id in the sync service's data.
    func centralManager(_ central: CBCentralManager, didDiscover p: CBPeripheral, advertisementData: [String: Any], rssi: NSNumber) {
        guard connecting != nil, let data = advertisementData[CBAdvertisementDataServiceDataKey] as? [CBUUID: Data],
              data.values.contains(where: { $0.range(of: wanted) != nil }) else { return }
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
        guard p == peripheral else { return }
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
        channel = ch; received = 0
        for s in [ch.inputStream as Stream, ch.outputStream as Stream] {
            s.delegate = self
            s.schedule(in: .main, forMode: .default)
            s.open()
        }
        connectTimeout?.cancel()
        var random = [UInt8](repeating: 0, count: 16)
        guard SecRandomCopyBytes(kSecRandomDefault, random.count, &random) == errSecSuccess else { return failConnect("no randomness") }
        let ms = Int(Date().timeIntervalSince(connectStart) * 1000)
        set("open", "\(name), PSM \(psm), \(ms) ms")
        connecting?.send(["ok": true, "name": name, "psm": psm, "ms": ms, "random": Data(random).base64EncodedString()]); connecting = nil
    }

    private func exchange(_ request: [String: Any], _ reply: ExactReply) {
        guard channel != nil else { return reply.fail("not connected") }
        guard let send = Data(base64Encoded: request["send"] as? String ?? "") else { return reply.fail("bad frames") }
        let after = (request["after_ms"] as? Double ?? 0) / 1000
        queue.append(Exchange(
            send: send, untilEnd: request["until"] as? String == "end", due: Date().addingTimeInterval(after),
            timeout: (request["timeout_ms"] as? Double ?? 15_000) / 1000, reply: reply))
        nextExchange()
    }

    private func nextExchange() {
        guard current == nil, let exchange = queue.first else { return }
        let wait = exchange.due.timeIntervalSinceNow
        if wait > 0 {
            DispatchQueue.main.asyncAfter(deadline: .now() + wait) { [weak self] in self?.nextExchange() }
            return
        }
        queue.removeFirst()
        current = exchange
        outbox = Array(exchange.send); collected = []; at = received
        // A late reply would be read as the next one's, so a timeout ends the channel.
        let timeout = DispatchWorkItem { [weak self] in self?.channelClosed("request timed out") }
        currentTimeout = timeout
        DispatchQueue.main.asyncAfter(deadline: .now() + exchange.timeout, execute: timeout)
        flush()
        receive()
    }

    private func flush() {
        guard let out = channel?.outputStream else { return }
        while !outbox.isEmpty, out.hasSpaceAvailable {
            let n = outbox.withUnsafeBufferPointer { out.write($0.baseAddress!, maxLength: $0.count) }
            if n <= 0 { break }
            outbox.removeFirst(n)
        }
    }

    /// Frames are a u16 LE length and that many bytes; each one counts, read or not.
    private func receive() {
        guard let input = channel?.inputStream else { return }
        var buffer = [UInt8](repeating: 0, count: 4096)
        while input.hasBytesAvailable {
            let n = input.read(&buffer, maxLength: buffer.count)
            if n <= 0 { break }
            inbox.append(contentsOf: buffer[0..<n])
        }
        while inbox.count >= 2 {
            let length = Int(inbox[0]) | Int(inbox[1]) << 8
            guard inbox.count >= 2 + length else { return }
            let frame = inbox[0..<(2 + length)]
            inbox.removeFirst(2 + length)
            received += 1
            guard let exchange = current else { continue }
            collected.append(contentsOf: frame)
            if !exchange.untilEnd || length == 16 { finishExchange() }
        }
    }

    private func finishExchange() {
        currentTimeout?.cancel(); currentTimeout = nil
        let exchange = current; current = nil
        exchange?.reply.send(["at": at, "frames": Data(collected).base64EncodedString(), "unix_ms": Date().timeIntervalSince1970 * 1000])
        collected = []
        nextExchange()
    }

    private func failExchanges(_ why: String) {
        currentTimeout?.cancel(); currentTimeout = nil
        current?.reply.fail(why); current = nil
        for exchange in queue { exchange.reply.fail(why) }
        queue = []; outbox = []; inbox = []; collected = []
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
