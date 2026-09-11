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

    def test_generated_source_uses_numeric_name_bytes(self):
        source = ble_config.render(1, b"private", [1, 2, 3, 4, 5, 6])
        self.assertIn("PROFILE: u8 = 1", source)
        self.assertIn("TARGET_NAME: &[u8] = &[112, 114", source)
        self.assertIn("Some([1, 2, 3, 4, 5, 6])", source)


if __name__ == "__main__":
    unittest.main()
