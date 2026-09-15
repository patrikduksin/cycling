"""Wi-Fi credentials must be validated, escaped, and stored privately."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import cycling_devtools.connectivity.wifi as wifi


class WifiConfigTests(unittest.TestCase):
    def config(self):
        return dict(ssid='Test "network"\\name', password='test-password', security='wpa-psk')

    def test_rejects_invalid_credentials(self):
        for field, value in [('ssid', 'é' * 17), ('password', '<hidden>'), ('password', 'short'),
                             ('security', 'wpa-eap')]:
            config = self.config()
            config[field] = value
            with self.assertRaises(ValueError):
                wifi.validate(config)

    def test_profile_is_private_and_runtime_command_encodes_input(self):
        with tempfile.TemporaryDirectory() as directory:
            local = Path(directory)
            with patch.object(wifi, 'LOCAL', local), patch.object(wifi, 'CONFIG', local / 'config.json'):
                config = self.config()
                wifi.save(wifi.CONFIG, json.dumps(config))
                command = wifi.command(config)
                self.assertNotIn(config['password'], command)
                self.assertEqual(command.split()[3:], [config['ssid'].encode().hex(), config['password'].encode().hex()])
                self.assertEqual(wifi.CONFIG.stat().st_mode & 0o777, 0o600)
                self.assertEqual(local.stat().st_mode & 0o777, 0o700)


if __name__ == '__main__':
    unittest.main()
