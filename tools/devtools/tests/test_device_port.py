import argparse
import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import cycling_devtools.device.port as device_port


class DevicePortTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.sysfs = self.root / 'class/tty'
        self.sysfs.mkdir(parents=True)
        self.manifest = self.root / 'manifest.json'
        self.manifest.write_text(json.dumps({'identity': '01:23:45:67:89:ab'}))

    def add_tty(self, name, serial='01-23-45-67-89-AB', vendor='303a', product='1001'):
        usb = self.root / 'devices' / name
        tty = usb / 'interface/tty' / name
        tty.mkdir(parents=True)
        (usb / 'idVendor').write_text(vendor + '\n')
        (usb / 'idProduct').write_text(product + '\n')
        if serial is not None:
            (usb / 'serial').write_text(serial + '\n')
        (self.sysfs / name).symlink_to(tty, target_is_directory=True)
        return usb

    def resolve(self):
        return device_port.resolve_port(self.sysfs, self.manifest)

    def test_renumbered_device_matches_ancestors_and_normalized_identity(self):
        self.add_tty('ttyACM0', serial='aa:bb:cc:dd:ee:ff')
        self.add_tty('ttyACM7')
        self.assertEqual(self.resolve(), Path('/dev/ttyACM7'))

    def test_zero_and_ambiguous_matches_fail_without_private_identity(self):
        for count in (0, 2):
            if count:
                self.add_tty('ttyACM1')
                self.add_tty('ttyACM2')
            with self.assertRaises(device_port.DiscoveryError) as caught:
                self.resolve()
            self.assertNotIn('0123456789ab', str(caught.exception))
            self.assertNotIn('01:23:45:67:89:ab', str(caught.exception))

    def test_vendor_product_missing_serial_and_malformed_identity_never_match(self):
        self.add_tty('ttyACM0', vendor='1234')
        self.add_tty('ttyACM1', product='0001')
        self.add_tty('ttyACM2', serial=None)
        self.add_tty('ttyACM3', serial='01:23:45:67:89:ab-private')
        with self.assertRaises(device_port.DiscoveryError):
            self.resolve()

    def test_hub_identity_cannot_override_mismatching_child(self):
        self.add_tty('ttyACM0', vendor='1234')
        hub = self.root / 'devices'
        for key, value in {'idVendor': '303a', 'idProduct': '1001', 'serial': '0123456789ab'}.items():
            (hub / key).write_text(value)
        with self.assertRaises(device_port.DiscoveryError):
            self.resolve()

    def test_manifest_errors_and_cli_failure_are_sanitized(self):
        self.manifest.write_text('{private broken manifest')
        with self.assertRaises(device_port.DiscoveryError) as caught:
            self.resolve()
        self.assertNotIn('broken', str(caught.exception))
        output, errors = io.StringIO(), io.StringIO()
        with (
            patch.object(device_port, 'resolve_port', side_effect=caught.exception),
            contextlib.redirect_stdout(output),
            contextlib.redirect_stderr(errors),
        ):
            self.assertEqual(device_port.main(), 1)
        self.assertEqual(output.getvalue(), '')
        self.assertNotIn('broken', errors.getvalue())

    def test_sysfs_enumeration_failure_is_sanitized(self):
        with patch.object(Path, 'glob', side_effect=OSError('private sysfs detail')):
            with self.assertRaises(device_port.DiscoveryError) as caught:
                self.resolve()
        self.assertNotIn('private', str(caught.exception))

    def test_cli_prints_only_verified_port(self):
        output = io.StringIO()
        with (
            patch.object(device_port, 'resolve_port', return_value=Path('/dev/ttyACM7')),
            contextlib.redirect_stdout(output),
        ):
            self.assertEqual(device_port.main(), 0)
        self.assertEqual(output.getvalue(), '/dev/ttyACM7\n')


class PortSelectionTests(unittest.TestCase):
    def test_explicit_then_environment_then_verified_discovery(self):
        discovered = Path('/dev/ttyACM7')
        resolve = Mock(return_value=discovered)
        environ = {'CYCLING_PORT': '/dev/ttyACM3'}
        self.assertEqual(device_port.select_port('/dev/ttyACM1', environ, resolve), '/dev/ttyACM1')
        self.assertEqual(device_port.select_port(None, environ, resolve), '/dev/ttyACM3')
        resolve.assert_not_called()
        for unset in ({}, {'CYCLING_PORT': ''}):
            self.assertEqual(device_port.select_port(None, unset, resolve), str(discovered))

    def test_missing_or_ambiguous_discovery_fails_the_command_clearly(self):
        parser = argparse.ArgumentParser()
        device_port.add_port_argument(parser)
        args = parser.parse_args([])
        errors = io.StringIO()
        failure = device_port.DiscoveryError('expected exactly one C606 matching the private backup manifest')
        with (
            patch.dict('os.environ', {}, clear=True),
            patch.object(device_port, 'resolve_port', side_effect=failure),
            contextlib.redirect_stderr(errors),
            self.assertRaises(SystemExit) as caught,
        ):
            device_port.selected_port(parser, args.port)
        self.assertEqual(caught.exception.code, 2)
        self.assertIn('expected exactly one C606', errors.getvalue())
        self.assertIn('--port or set CYCLING_PORT', errors.getvalue())

    def test_no_tool_hard_codes_a_default_tty(self):
        package = Path(device_port.__file__).resolve().parents[1]
        offenders = [
            path.name
            for path in package.rglob('*.py')
            if "'/dev/ttyACM" in path.read_text() or '"/dev/ttyACM' in path.read_text()
        ]
        self.assertEqual(offenders, [])


if __name__ == '__main__':
    unittest.main()
