"""Tool discovery must not initialize optional hardware integrations."""
import os
from pathlib import Path
import subprocess
import sys
import unittest

SCRIPTS = Path(__file__).resolve().parent


class EntrypointTests(unittest.TestCase):
    def test_bluetooth_import_and_help_without_dbus_dependencies(self):
        # -S excludes site packages, including D-Bus and GLib. Import and help
        # must succeed without even loading their hardware integration modules.
        for name in ('bluetooth', 'ble_sensor_sim'):
            for args in (['-c', f'import {name}'], [str(SCRIPTS / f'{name}.py'), '--help']):
                result = subprocess.run([sys.executable, '-S', *args], cwd=SCRIPTS,
                                        capture_output=True, text=True,
                                        env=dict(os.environ, CYCLING_SIM_PROFILE='invalid'))
                self.assertEqual(result.returncode, 0, result.stderr)
                if '--help' in args:
                    self.assertIn('usage:', result.stdout)


if __name__ == '__main__':
    unittest.main()
