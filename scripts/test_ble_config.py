import unittest

import ble_config


class BleConfigTest(unittest.TestCase):
    def test_echo_and_simulator_modes_have_no_private_address(self):
        self.assertEqual(ble_config.configuration("echo"), (0, b"", None))
        self.assertEqual(ble_config.configuration("sim-heart"), (1, b"Cycling Sim", None))
        self.assertEqual(ble_config.configuration("sim-csc"), (2, b"Cycling Sim", None))

    def test_authorized_peer_is_validated_and_address_uses_hci_order(self):
        record = {
            "authorized_by_user": True,
            "profile": "Heart Rate Service",
            "name": "Owned fixture",
            "address": "01:23:45:67:89:AB",
        }
        self.assertEqual(
            ble_config.configuration("authorized-heart", record),
            (1, b"Owned fixture", [0xAB, 0x89, 0x67, 0x45, 0x23, 0x01]),
        )
        record["authorized_by_user"] = False
        with self.assertRaises(ValueError):
            ble_config.configuration("authorized-heart", record)

    def test_invalid_private_address_is_not_repeated_in_errors(self):
        with self.assertRaises(ValueError) as raised:
            ble_config.address_bytes("private:23:45:67:89:AB")
        self.assertNotIn("private", str(raised.exception))

    def test_runtime_command_uses_bounded_hex_fields(self):
        self.assertEqual(ble_config.command("sim-heart"), "BLE SELECT HRS 4379636c696e672053696d -")
        self.assertEqual(ble_config.command("echo"), "BLE FORGET")


if __name__ == "__main__":
    unittest.main()
