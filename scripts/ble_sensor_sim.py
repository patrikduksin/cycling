#!/usr/bin/python3
"""Owned deterministic HRS/CSC simulator for the C606 central-role test."""
import os
import signal

import dbus
import dbus.mainloop.glib
import dbus.service
from gi.repository import GLib

BLUEZ = "org.bluez"
OM = "org.freedesktop.DBus.ObjectManager"
PROPS = "org.freedesktop.DBus.Properties"
GATT_MANAGER = "org.bluez.GattManager1"
ADV_MANAGER = "org.bluez.LEAdvertisingManager1"
SERVICE = "org.bluez.GattService1"
CHAR = "org.bluez.GattCharacteristic1"
ADV = "org.bluez.LEAdvertisement1"
ROOT = "/com/cycling/simulator"
PROFILE = os.environ.get("CYCLING_SIM_PROFILE", "both")
if PROFILE not in {"heart", "csc", "both"}:
    raise SystemExit("CYCLING_SIM_PROFILE must be heart, csc, or both")


class Application(dbus.service.Object):
    def __init__(self, bus):
        super().__init__(bus, ROOT)
        self.services = []
        if PROFILE in {"heart", "both"}:
            self.services.append(HeartRateService(bus, len(self.services)))
        if PROFILE in {"csc", "both"}:
            self.services.append(CscService(bus, len(self.services)))

    @dbus.service.method(OM, out_signature="a{oa{sa{sv}}}")
    def GetManagedObjects(self):
        result = {}
        for service in self.services:
            result[service.path] = service.properties()
            for characteristic in service.characteristics:
                result[characteristic.path] = characteristic.properties()
        return result


class ServiceObject(dbus.service.Object):
    def __init__(self, bus, index, uuid):
        self.path = f"{ROOT}/service{index}"
        self.uuid = uuid
        self.characteristics = []
        super().__init__(bus, self.path)

    def properties(self):
        return {SERVICE: {"UUID": self.uuid, "Primary": dbus.Boolean(True)}}

    @dbus.service.method(PROPS, in_signature="s", out_signature="a{sv}")
    def GetAll(self, interface):
        if interface != SERVICE:
            raise dbus.exceptions.DBusException("unsupported interface")
        return self.properties()[SERVICE]


class CharacteristicObject(dbus.service.Object):
    def __init__(self, bus, service, index, uuid, flags):
        self.path = f"{service.path}/char{index}"
        self.service = service
        self.uuid = uuid
        self.flags = flags
        self.notifying = False
        super().__init__(bus, self.path)

    def properties(self):
        return {CHAR: {
            "Service": dbus.ObjectPath(self.service.path),
            "UUID": self.uuid,
            "Flags": dbus.Array(self.flags, signature="s"),
            "Notifying": dbus.Boolean(self.notifying),
        }}

    @dbus.service.method(PROPS, in_signature="s", out_signature="a{sv}")
    def GetAll(self, interface):
        if interface != CHAR:
            raise dbus.exceptions.DBusException("unsupported interface")
        return self.properties()[CHAR]

    @dbus.service.signal(PROPS, signature="sa{sv}as")
    def PropertiesChanged(self, interface, changed, invalidated):
        pass


class FixedValue(CharacteristicObject):
    def __init__(self, bus, service, index, uuid, value):
        self.value = value
        super().__init__(bus, service, index, uuid, ["read"])

    @dbus.service.method(CHAR, in_signature="a{sv}", out_signature="ay")
    def ReadValue(self, options):
        return dbus.Array(self.value, signature="y")


class Measurement(CharacteristicObject):
    def __init__(self, bus, service, index, uuid):
        self.tick = 0
        self.timer = None
        super().__init__(bus, service, index, uuid, ["notify"])

    def value(self):
        raise NotImplementedError

    def emit_value(self):
        if not self.notifying:
            self.timer = None
            return False
        self.PropertiesChanged(CHAR, {"Value": dbus.Array(self.value(), signature="y")}, [])
        self.tick += 1
        return True

    @dbus.service.method(CHAR)
    def StartNotify(self):
        if self.notifying:
            return
        self.tick = 0
        self.notifying = True
        self.PropertiesChanged(CHAR, {"Notifying": dbus.Boolean(True)}, [])
        self.timer = GLib.timeout_add(500, self.emit_value)

    @dbus.service.method(CHAR)
    def StopNotify(self):
        self.notifying = False
        if self.timer is not None:
            GLib.source_remove(self.timer)
            self.timer = None
        self.PropertiesChanged(CHAR, {"Notifying": dbus.Boolean(False)}, [])


class HeartMeasurement(Measurement):
    def value(self):
        bpm = 72 + self.tick % 2
        if self.tick % 2:
            return [0x1e, bpm, 0x10, 0x00, 0x00, 0x04]
        return [0x06, bpm]


class CscMeasurement(Measurement):
    def value(self):
        revolutions = (100 + self.tick) & 0xffff
        event_time = (1024 + self.tick * 1024) & 0xffff
        return [0x02, revolutions & 0xff, revolutions >> 8,
                event_time & 0xff, event_time >> 8]


class HeartRateService(ServiceObject):
    def __init__(self, bus, index):
        super().__init__(bus, index, "180D")
        self.characteristics = [
            HeartMeasurement(bus, self, 0, "2A37"),
            FixedValue(bus, self, 1, "2A38", [1]),
        ]


class CscService(ServiceObject):
    def __init__(self, bus, index):
        super().__init__(bus, index, "1816")
        self.characteristics = [
            CscMeasurement(bus, self, 0, "2A5B"),
            FixedValue(bus, self, 1, "2A5C", [2, 0]),
        ]


class Advertisement(dbus.service.Object):
    path = f"{ROOT}/advertisement0"

    def __init__(self, bus):
        super().__init__(bus, self.path)

    @dbus.service.method(PROPS, in_signature="s", out_signature="a{sv}")
    def GetAll(self, interface):
        if interface != ADV:
            raise dbus.exceptions.DBusException("unsupported interface")
        return {
            "Type": "peripheral",
            "ServiceUUIDs": dbus.Array(
                (["180D"] if PROFILE == "heart" else ["1816"] if PROFILE == "csc" else ["180D", "1816"]),
                signature="s",
            ),
            "LocalName": "Cycling Sim",
        }

    @dbus.service.method(ADV)
    def Release(self):
        pass


dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
bus = dbus.SystemBus()
managed = dbus.Interface(bus.get_object(BLUEZ, "/"), OM).GetManagedObjects()
adapter = next(path for path, interfaces in managed.items()
               if GATT_MANAGER in interfaces and ADV_MANAGER in interfaces)
gatt = dbus.Interface(bus.get_object(BLUEZ, adapter), GATT_MANAGER)
advertising = dbus.Interface(bus.get_object(BLUEZ, adapter), ADV_MANAGER)
application = Application(bus)
advertisement = Advertisement(bus)
loop = GLib.MainLoop()
registered = {"gatt": False, "advertisement": False}


def ready(label):
    registered[label] = True
    if all(registered.values()):
        print(f"CYCLING_SIM ready profile={PROFILE}", flush=True)


def failed(error):
    print(f"CYCLING_SIM failed type={error.get_dbus_name()}", flush=True)
    loop.quit()


def shutdown(*unused):
    if registered["advertisement"]:
        try:
            advertising.UnregisterAdvertisement(advertisement.path)
        except dbus.DBusException:
            pass
    if registered["gatt"]:
        try:
            gatt.UnregisterApplication(ROOT)
        except dbus.DBusException:
            pass
    loop.quit()
    return False


gatt.RegisterApplication(ROOT, {}, reply_handler=lambda: ready("gatt"), error_handler=failed)
advertising.RegisterAdvertisement(advertisement.path, {},
                                  reply_handler=lambda: ready("advertisement"), error_handler=failed)
GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGTERM, shutdown)
GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGINT, shutdown)
loop.run()
