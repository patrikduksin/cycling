"""The single .local guard and shared reply/byte helpers."""
from pathlib import Path
import stat
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from cycling_devtools import workspace
from cycling_devtools.little_endian import u16, u32, u64
from cycling_devtools.terminal.usb import reply_fields
from workspace_fixture import private_root


class PrivatePathTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        context = private_root(self.root)
        self.local = context.__enter__()
        self.addCleanup(context.__exit__, None, None, None)

    def test_accepts_new_and_nested_children_of_local(self):
        self.assertEqual(workspace.private_path(self.local / 'a' / 'b.json'), self.local / 'a' / 'b.json')

    def test_rejects_local_itself_outside_paths_and_traversal(self):
        for value in (self.local, self.root, self.root / 'public.img', self.local / '..' / 'escape'):
            with self.subTest(value=value), self.assertRaises(ValueError):
                workspace.private_path(value)

    def test_rejects_symlink_escape(self):
        (self.local / 'escape').symlink_to(self.root, target_is_directory=True)
        with self.assertRaises(ValueError):
            workspace.private_path(self.local / 'escape' / 'public.img')

    def test_symlinked_local_directory_fails_closed(self):
        # Git cannot confirm ignore rules beyond a symlink, so this is refused.
        elsewhere = self.root / 'elsewhere'
        elsewhere.mkdir()
        self.local.rmdir()
        self.local.symlink_to(elsewhere, target_is_directory=True)
        with self.assertRaises(ValueError):
            workspace.private_path(self.local / 'x')

    def test_rejects_paths_git_does_not_ignore(self):
        (self.root / '.gitignore').write_text('/.local/*\n!/.local/published\n')
        workspace.private_path(self.local / 'evidence')
        with self.assertRaisesRegex(ValueError, 'git-ignored'):
            workspace.private_path(self.local / 'published')

    def test_fails_closed_without_git(self):
        with patch.object(subprocess, 'run', side_effect=FileNotFoundError('git')), \
                self.assertRaisesRegex(ValueError, 'git-ignored'):
            workspace.private_path(self.local / 'evidence')

    def test_private_directory_is_new_and_owner_only(self):
        created = workspace.private_directory(self.local / 'run' / 'one')
        self.assertEqual(stat.S_IMODE(created.stat().st_mode), 0o700)
        with self.assertRaises(FileExistsError):
            workspace.private_directory(created)
        with self.assertRaises(ValueError):
            workspace.private_directory(self.root / 'public')
        self.assertFalse((self.root / 'public').exists())


class HelperTests(unittest.TestCase):
    def test_reply_fields_keep_empty_values_and_can_reject_duplicates(self):
        self.assertEqual(reply_fields('state=4 error= note=a=b'), {'state': '4', 'error': '', 'note': 'a=b'})
        self.assertEqual(reply_fields('a=1 a=2'), {'a': '2'})
        with self.assertRaisesRegex(ValueError, 'duplicate'):
            reply_fields('a=1 a=2', unique=True)

    def test_little_endian_fields_are_bounds_checked(self):
        data = bytes(range(1, 9))
        self.assertEqual(u16(data, 6), 0x0807)
        self.assertEqual(u32(data, 0), 0x04030201)
        self.assertEqual(u64(data, 0), 0x0807060504030201)
        for reader, offset in ((u16, 7), (u32, 5), (u64, 1), (u16, -1)):
            with self.subTest(reader=reader.__name__, offset=offset), self.assertRaises(ValueError):
                reader(data, offset)


if __name__ == '__main__':
    unittest.main()
