"""Wi-Fi credentials must be validated, escaped, and stored privately."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import wifi


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

    def test_generated_source_is_private_and_escapes_all_input(self):
        with tempfile.TemporaryDirectory() as directory:
            local = Path(directory)
            with patch.object(wifi, 'LOCAL', local), patch.object(wifi, 'CONFIG', local / 'config.json'):
                config = self.config()
                wifi.save(wifi.CONFIG, json.dumps(config))
                wifi.generate()
                result = (local / 'config.rs').read_text()
                self.assertNotIn(config['password'], result)
                self.assertIn('pub const SSID: &str = ' + wifi.rust_string(config['ssid']), result)
                self.assertEqual((local / 'config.rs').stat().st_mode & 0o777, 0o600)
                self.assertEqual(wifi.CONFIG.stat().st_mode & 0o777, 0o600)
                self.assertEqual(local.stat().st_mode & 0o777, 0o700)

    def test_missing_config_builds_without_a_network(self):
        with tempfile.TemporaryDirectory() as directory:
            local = Path(directory)
            with patch.object(wifi, 'LOCAL', local), patch.object(wifi, 'CONFIG', local / 'missing.json'):
                wifi.generate()
                source = (local / 'config.rs').read_text()
                self.assertIn('pub const SSID: &str = "";', source)


if __name__ == '__main__':
    unittest.main()
