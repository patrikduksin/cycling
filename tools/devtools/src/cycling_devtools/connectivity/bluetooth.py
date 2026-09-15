#!/usr/bin/python3
"""Verify owned C606 BLE echo notifications, readback and reconnects."""
import argparse
import json
import time


BLUEZ = "org.bluez"
DEVICE = "org.bluez.Device1"
CHAR = "org.bluez.GattCharacteristic1"
PROPS = "org.freedesktop.DBus.Properties"
MANAGER = "org.freedesktop.DBus.ObjectManager"
SERVICE_UUID = "7e570001-2a6f-4d75-9f6a-5afbf0f50c06"
CHAR_UUID = "7e570002-2a6f-4d75-9f6a-5afbf0f50c06"


def objects():
    return manager.GetManagedObjects()


def owned_device():
    matches = []
    for path, interfaces in objects().items():
        values = interfaces.get(DEVICE)
        if values and "Cycling Echo" in (str(values.get("Name", "")), str(values.get("Alias", ""))):
            matches.append(path)
    if len(matches) != 1:
        raise RuntimeError(f"expected one owned device, found {len(matches)}")
    return matches[0]


def discover_owned():
    adapter_path = next(
        path for path, interfaces in objects().items() if "org.bluez.Adapter1" in interfaces
    )
    adapter = dbus.Interface(bus.get_object(BLUEZ, adapter_path), "org.bluez.Adapter1")
    adapter.SetDiscoveryFilter(dbus.Dictionary({"Transport": "le"}, signature="sv"))
    adapter.StartDiscovery()
    try:
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            try:
                return owned_device()
            except RuntimeError:
                time.sleep(.1)
        raise TimeoutError("owned advertisement not found")
    finally:
        adapter.StopDiscovery()


def wait_resolved(device_path, seconds=10):
    props = dbus.Interface(bus.get_object(BLUEZ, device_path), PROPS)
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if bool(props.Get(DEVICE, "ServicesResolved")):
            return
        time.sleep(.1)
    raise TimeoutError("services did not resolve")


def characteristic(device_path):
    matches = []
    for path, interfaces in objects().items():
        values = interfaces.get(CHAR)
        if values and str(values.get("UUID", "")).lower() == CHAR_UUID and path.startswith(device_path):
            service_path = str(values.get("Service", ""))
            service = objects().get(service_path, {}).get("org.bluez.GattService1", {})
            if str(service.get("UUID", "")).lower() == SERVICE_UUID:
                matches.append(path)
    if len(matches) != 1:
        raise RuntimeError(f"expected one echo characteristic, found {len(matches)}")
    return matches[0]


def connect(device_path):
    device = dbus.Interface(bus.get_object(BLUEZ, device_path), DEVICE)
    last = None
    for _ in range(3):
        try:
            device.Connect(timeout=10)
            wait_resolved(device_path)
            return device
        except (dbus.DBusException, TimeoutError) as error:
            last = error
            try:
                device.Disconnect()
            except dbus.DBusException:
                pass
            time.sleep(1)
    raise last


def exchange(device_path, round_number, payload):
    device = connect(device_path)
    receiver = None
    echo = None
    notifying = False
    notified = []

    def changed(interface, values, invalidated):
        if str(interface) == CHAR and "Value" in values:
            notified.append(bytes(values["Value"]))

    try:
        char_path = characteristic(device_path)
        echo = dbus.Interface(bus.get_object(BLUEZ, char_path), CHAR)
        mtu = int(dbus.Interface(bus.get_object(BLUEZ, char_path), PROPS).Get(CHAR, "MTU"))
        receiver = bus.add_signal_receiver(
            changed,
            signal_name="PropertiesChanged",
            dbus_interface=PROPS,
            path=char_path,
        )
        echo.StartNotify()
        notifying = True
        echo.WriteValue(dbus.Array(payload, signature="y"), dbus.Dictionary({}, signature="sv"))
        deadline = time.monotonic() + 5
        context = GLib.MainContext.default()
        while payload not in notified and time.monotonic() < deadline:
            while context.pending():
                context.iteration(False)
            time.sleep(.01)
        if payload not in notified:
            raise TimeoutError("exact notification missing")
        readback = bytes(echo.ReadValue(dbus.Dictionary({}, signature="sv")))
        if readback != payload:
            raise AssertionError("readback mismatch")
        try:
            echo.WriteValue(dbus.Array([0x99], signature="y"), dbus.Dictionary({}, signature="sv"))
        except dbus.DBusException:
            short_rejected = True
        else:
            short_rejected = False
        if not short_rejected:
            raise AssertionError("short write was accepted")
        if bytes(echo.ReadValue(dbus.Dictionary({}, signature="sv"))) != payload:
            raise AssertionError("rejected write changed readback")
        echo.StopNotify()
        notifying = False
        device.Disconnect()
        time.sleep(.5)
        print(json.dumps({"round": round_number, "notification": True, "readback": True,
                          "short_rejected": True, "mtu": mtu}))
    finally:
        if notifying and echo is not None:
            try:
                echo.StopNotify()
            except dbus.DBusException:
                pass
        if receiver is not None:
            receiver.remove()
        try:
            device.Disconnect()
        except dbus.DBusException:
            pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.parse_args()
    global dbus, GLib, bus, manager
    import dbus
    import dbus.mainloop.glib
    from gi.repository import GLib

    dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
    bus = dbus.SystemBus()
    manager = dbus.Interface(bus.get_object(BLUEZ, "/"), MANAGER)
    device_path = discover_owned()
    exchange(device_path, 1, bytes([1, 0x43, 0x36, 0x30, 0x36, 0xaa, 0x55, 0x7e]))
    exchange(device_path, 2, bytes([2, 0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70]))


if __name__ == '__main__':
    main()
