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

    func status() -> [String: Any] {
        ["state": state, "detail": detail, "name": name, "psm": psm]
    }

    func later(_ request: [String: Any], reply: ExactReply) {
        DispatchQueue.main.async { [self] in
            switch request["op"] as? String {
            case "status": reply.send(status())
            case "connect": connect(reply)
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

    private func closeChannel() {
        if let ch = channel {
            for s in [ch.inputStream as Stream, ch.outputStream as Stream] { s.close(); s.remove(from: .main, forMode: .default) }
        }
        channel = nil
    }

    func stream(_ s: Stream, handle event: Stream.Event) {
        switch event {
        case .errorOccurred, .endEncountered:
            closeChannel()
            set("failed", "channel closed")
        default: break
        }
    }
}
let exactModule: ExactModule.Type = VanaLink.self
