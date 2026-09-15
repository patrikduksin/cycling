"""Protocol corruption, interrupted backup, and write authorization checks."""
import hashlib
import json
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch

import cycling_devtools.device.mmc_maintenance as mm


def frame(ident=1, start=0, count=1, encoding=0, data=None, status=0, checksum=None):
    if data is None:
        data = b'x' * (count * 512)
    wire = data
    if encoding == 1:
        wire = data[:1]
    elif encoding == 3:
        wire = b''
    if checksum is None:
        checksum = mm.crc(data)
    header = mm.HEADER.pack(b'C6RP', ident, status, start, count, encoding, checksum, 0)
    return header[:28] + struct.pack('<I', mm.crc(header[:28])) + wire


class Wire(mm.Connection):
    def __init__(self, incoming):
        super().__init__('unused')
        self.incoming = incoming
        self.outgoing = []
        self.sequence = 0
        self.total, self.token = 20, 123

    def send(self, data):
        self.outgoing.append(data)

    def receive(self, size, deadline=None):
        if len(self.incoming) < size:
            raise ConnectionError('truncated frame')
        result, self.incoming = self.incoming[:size], self.incoming[size:]
        return result


class Medium:
    def __init__(self, data):
        self.data = data
        self.total = len(data) // 512
        self.calls = []
        self.fail_second = False
        self.reads = 0

    def bounds(self, start, count):
        if count <= 0 or start < 0 or start + count > self.total:
            raise ValueError('bounds')

    def read(self, start, count):
        self.reads += 1
        data = self.data[start * 512:(start + count) * 512]
        if self.fail_second and self.reads == 2:
            data = b'!' + data[1:]
        for offset in range(0, len(data), 1024):
            yield data[offset:offset + 1024]

    def arm(self, start, count):
        self.calls.append(('arm', start, count))

    def write_sector(self, start, data):
        self.calls.append(('write', start, data))


class ProtocolTests(unittest.TestCase):
    def test_request_crc_and_payload(self):
        wire = Wire(b'')
        wire.request(4, 3, 1, 123, b'a' * 512)
        request = wire.outgoing[0]
        fields = mm.HEADER.unpack(request[:32])
        self.assertEqual(fields[:6], (b'C6RQ', 4, 1, 3, 1, 123))
        self.assertEqual(fields[6], mm.crc(request[32:]))
        self.assertEqual(fields[7], mm.crc(request[:28]))

    def test_contiguous_raw_and_uniform_frames(self):
        wire = Wire(frame(count=2, data=b'a' * 1024) + frame(start=2, count=3, encoding=1, data=b'\0' * 1536))
        self.assertEqual(b''.join(wire.read(0, 5)), b'a' * 1024 + b'\0' * 1536)
        self.assertEqual(len(wire.outgoing), 1)

    def test_wrong_id_header_crc_and_payload_crc(self):
        for incoming in (frame(ident=2), bytes([0]) + frame()[1:], frame(checksum=42)):
            with self.subTest(incoming=incoming[:12]):
                with self.assertRaises(ValueError):
                    list(Wire(incoming).read(0, 1))

    def test_missing_overlapping_and_excess_read_ranges(self):
        for incoming in (frame(start=1), frame(count=2), frame(encoding=3),
                         frame() + frame(start=0)):
            wire = Wire(incoming)
            with self.subTest(incoming=incoming[:28]):
                with self.assertRaises(ValueError):
                    list(wire.read(0, 2 if len(incoming) > 1056 else 1))

    def test_frame_limit_and_unknown_encoding(self):
        for incoming in (frame(count=8), frame(encoding=4)):
            with self.assertRaises(ValueError):
                list(Wire(incoming).read(0, 10))

    def test_device_error_and_truncation_do_not_replay(self):
        for incoming in (frame(status=5, count=0, encoding=3), frame()[:-1]):
            wire = Wire(incoming)
            with self.assertRaises((RuntimeError, ConnectionError)):
                list(wire.read(0, 1))
            self.assertEqual(len(wire.outgoing), 1)

    def test_info_and_ack(self):
        data = struct.pack('<QII', 7733248, 512, 123)
        wire = Wire(frame(count=0, encoding=2, data=data))
        self.assertEqual(wire.info()['bytes'], 3959422976)
        wire = Wire(frame(start=2, count=1, encoding=3, checksum=mm.crc(b'a' * 512)))
        wire.write_sector(2, b'a' * 512)
        wire = Wire(frame(start=2, count=1, encoding=3, checksum=5))
        with self.assertRaises(ValueError):
            wire.write_sector(2, b'a' * 512)
        self.assertEqual(len(wire.outgoing), 1)

    def test_initial_info_skips_boot_text_and_stale_reply(self):
        data = struct.pack('<QII', 20, 512, 123)
        wire = Wire(b'boot started\r\n' + frame(ident=100, count=0, encoding=2, data=data)
                    + frame(count=0, encoding=2, data=data))
        self.assertEqual(wire.info()['total_sectors'], 20)
        self.assertEqual(len(wire.outgoing), 1)

    def test_initial_info_resynchronization_is_bounded(self):
        wire = Wire(b'x' * 5000)
        with self.assertRaisesRegex(ValueError, '4096'):
            wire.info()

    def test_new_connection_request_ids_use_fresh_entropy(self):
        with patch.object(mm.secrets, 'randbelow', side_effect=[21, 9823]):
            first, second = mm.Connection('unused'), mm.Connection('unused')
        self.assertEqual((first.sequence, second.sequence), (22, 9824))

    def test_recover_does_not_require_info_or_token(self):
        wire = Wire(frame(count=0, encoding=3))
        wire.token = wire.total = None
        wire.recover()
        self.assertEqual(mm.HEADER.unpack(wire.outgoing[0])[1], 5)

    def test_wide_is_explicit_zero_range_opcode_with_fresh_id(self):
        data = struct.pack('<QII', 20, 512, 123)
        wire = Wire(frame(count=0, encoding=2, data=data)
                    + frame(ident=2, count=0, encoding=3))
        wire.info()
        self.assertEqual(len(wire.outgoing), 1)
        wire.wide()
        fields = mm.HEADER.unpack(wire.outgoing[1])
        self.assertEqual(fields[:7], (b'C6RQ', 6, 2, 0, 0, 0, 0))
        self.assertEqual(fields[7], mm.crc(wire.outgoing[1][:28]))

    def test_wide_requires_successful_session_info(self):
        wire = Wire(b'')
        wire.token = wire.total = None
        with self.assertRaisesRegex(ValueError, 'successful INFO'):
            wire.wide()
        self.assertEqual(wire.outgoing, [])

    def test_wide_failure_or_invalid_ack_stops_without_retry(self):
        for incoming in (frame(status=7, count=0, encoding=3),
                         frame(start=1, count=0, encoding=3),
                         frame(ident=2, count=0, encoding=3), b''):
            wire = Wire(incoming)
            with self.assertRaises((ValueError, RuntimeError, ConnectionError)):
                wire.wide()
            self.assertEqual(len(wire.outgoing), 1)

    def test_bounds_reject_before_request(self):
        wire = Wire(b'')
        for start, count in ((-1, 1), (20, 1), (0, 0), (19, 2)):
            with self.assertRaises(ValueError):
                list(wire.read(start, count))
        self.assertEqual(wire.outgoing, [])

    def test_partial_os_writes_continue_remaining_bytes(self):
        connection = mm.Connection('unused')
        connection.fd = 10
        writes = []
        def partial(fd, data):
            writes.append(data)
            return min(3, len(data))
        with patch.object(mm.select, 'select', return_value=([], [10], [])), patch.object(mm.os, 'write', side_effect=partial):
            connection.send(b'abcdefgh')
        self.assertEqual(writes, [b'abcdefgh', b'defgh', b'gh'])

    def test_disconnect_does_not_reopen_or_resend(self):
        connection = mm.Connection('unused')
        connection.fd = 10
        with patch.object(mm.select, 'select', return_value=([10], [], [])), patch.object(mm.os, 'read', return_value=b''):
            with self.assertRaises(ConnectionError):
                connection.receive(32)


class BackupTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.local = self.root / '.local'
        self.local.mkdir()
        self.root_patch = patch.object(mm, 'ROOT', self.root)
        self.root_patch.start()
        self.addCleanup(self.root_patch.stop)
        self.addCleanup(self.temp.cleanup)

    def test_two_pass_backup_and_preserved_existing_image(self):
        medium = Medium(b'\0' * 1024 + b'x' * 512)
        image = self.local / 'backup.img'
        result = mm.backup(medium, image)
        self.assertEqual(medium.reads, 2)
        self.assertTrue(result['complete'] and result['verified'])
        self.assertEqual(image.read_bytes(), medium.data)
        self.assertEqual(result['sha256'], hashlib.sha256(medium.data).hexdigest())
        mm.validate_backup(Path(str(image) + '.json'), medium.total)
        with self.assertRaises(ValueError):
            mm.backup(medium, image)
        self.assertEqual(image.read_bytes(), medium.data)

    def test_second_read_mismatch_leaves_manifest_unverified(self):
        medium = Medium(b'a' * 1024)
        medium.fail_second = True
        image = self.local / 'backup.img'
        with self.assertRaises(ValueError):
            mm.backup(medium, image)
        manifest = json.loads(Path(str(image) + '.json').read_text())
        self.assertTrue(manifest['complete'])
        self.assertFalse(manifest['verified'])
        with self.assertRaises(ValueError):
            mm.validate_backup(Path(str(image) + '.json'), medium.total)

    def test_interrupted_first_read_never_marks_complete(self):
        medium = Medium(b'a' * 1024)
        def interrupted(*args):
            yield b'a' * 512
            raise ConnectionError('interrupted')
        medium.read = interrupted
        image = self.local / 'backup.img'
        with self.assertRaises(ConnectionError):
            mm.backup(medium, image)
        manifest = json.loads(Path(str(image) + '.json').read_text())
        self.assertFalse(manifest['complete'])
        self.assertFalse(manifest['verified'])

    def interrupted_backup(self, data, prefix, image_name='resume.img'):
        medium = Medium(data)
        def interrupted(start, count):
            yield prefix
            raise ConnectionError('interrupted after complete frame')
        medium.read = interrupted
        image = self.local / image_name
        with self.assertRaises(ConnectionError):
            mm.backup(medium, image)
        return image

    def test_explicit_resume_preserves_sparse_zero_prefix_and_verifies_all(self):
        data = b'\0' * 1024 + b'x' * 1024
        image = self.interrupted_backup(data, data[:1024])
        self.assertEqual(image.stat().st_size, 1024)
        self.assertEqual(image.read_bytes(), b'\0' * 1024)
        medium = Medium(data)
        requests = []
        read = medium.read
        def recorded(start, count):
            requests.append((start, count))
            yield from read(start, count)
        medium.read = recorded
        result = mm.backup(medium, image, resume=True)
        self.assertEqual(requests, [(2, 2), (0, 4)])
        self.assertEqual(image.read_bytes(), data)
        self.assertTrue(result['verified'])

    def test_repeated_interruption_truncates_to_complete_sparse_frames(self):
        data = b'a' * 512 + b'\0' * 1536
        image = self.interrupted_backup(data, data[:512])
        medium = Medium(data)
        def interrupted(start, count):
            self.assertEqual((start, count), (1, 3))
            yield b'\0' * 1024
            raise TimeoutError('next response timed out')
        medium.read = interrupted
        with self.assertRaises(TimeoutError):
            mm.backup(medium, image, resume=True)
        self.assertEqual(image.stat().st_size, 1536)
        self.assertEqual(image.read_bytes(), data[:1536])
        self.assertFalse(json.loads(Path(str(image) + '.json').read_text())['complete'])

    def test_corrupt_prefix_prevents_verified_manifest_on_resume(self):
        data = b'a' * 1536
        image = self.interrupted_backup(data, data[:512])
        image.write_bytes(b'z' * 512)
        medium = Medium(data)
        with self.assertRaisesRegex(ValueError, 'differs from backup'):
            mm.backup(medium, image, resume=True)
        metadata = json.loads(Path(str(image) + '.json').read_text())
        self.assertTrue(metadata['complete'])
        self.assertFalse(metadata['verified'])
        self.assertEqual(image.read_bytes()[:512], b'z' * 512)

    def test_complete_unverified_resume_only_verifies_and_verified_refuses(self):
        data = b'a' * 1024
        medium = Medium(data)
        medium.fail_second = True
        image = self.local / 'complete.img'
        with self.assertRaises(ValueError):
            mm.backup(medium, image)
        medium = Medium(data)
        result = mm.backup(medium, image, resume=True)
        self.assertEqual(medium.reads, 1)
        self.assertTrue(result['verified'])
        before = image.read_bytes()
        with self.assertRaisesRegex(ValueError, 'unverified manifest'):
            mm.backup(medium, image, resume=True)
        self.assertEqual(medium.reads, 1)
        self.assertEqual(image.read_bytes(), before)

    def test_resume_rejects_bad_prefix_size_path_and_geometry_without_reads(self):
        data = b'a' * 1024
        image = self.interrupted_backup(data, data[:512])
        manifest_path = Path(str(image) + '.json')
        original = json.loads(manifest_path.read_text())
        medium = Medium(data)
        for size in (513, 1536):
            image.write_bytes(b'a' * size)
            with self.assertRaisesRegex(ValueError, 'whole-sector prefix'):
                mm.backup(medium, image, resume=True)
        image.write_bytes(b'a' * 512)
        for changes in ({'image': str(self.local / 'other.img')}, {'total_sectors': 5}, {'sector_size': 1024}):
            manifest_path.write_text(json.dumps({**original, **changes}))
            with self.assertRaisesRegex(ValueError, 'exact image and geometry'):
                mm.backup(medium, image, resume=True)
        self.assertEqual(medium.reads, 0)

    def test_complete_image_changed_on_disk_refuses_resume_before_media_read(self):
        data = b'a' * 1024
        medium = Medium(data)
        medium.fail_second = True
        image = self.local / 'changed.img'
        with self.assertRaises(ValueError):
            mm.backup(medium, image)
        image.write_bytes(b'z' * 1024)
        medium = Medium(data)
        with self.assertRaisesRegex(ValueError, 'recorded size or SHA256'):
            mm.backup(medium, image, resume=True)
        self.assertEqual(medium.reads, 0)

    def test_full_prefix_without_completion_manifest_runs_only_verification(self):
        data = b'a' * 1024
        image = self.interrupted_backup(data, data)
        medium = Medium(data)
        result = mm.backup(medium, image, resume=True)
        self.assertEqual(medium.reads, 1)
        self.assertTrue(result['complete'] and result['verified'])

    def test_single_read_copy_exception_is_explicit_complete_and_distinct(self):
        image = self.local / 'single.img'
        image.write_bytes(b'a' * 1024)
        copy = self.root / 'external-copy.img'
        copy.write_bytes(image.read_bytes())
        manifest = Path(str(image) + '.json')
        metadata = {'version': 1, 'image': str(image), 'total_sectors': 2,
                    'sector_size': 512, 'complete': True, 'verified': False,
                    'sha256': mm.digest_file(image)}
        manifest.write_text(json.dumps(metadata))
        with self.assertRaises(ValueError):
            mm.validate_backup(manifest, 2)
        self.assertEqual(mm.validate_backup(manifest, 2, single_read_copy=copy),
                         'single_media_read_with_matching_copy')
        self.assertFalse(json.loads(manifest.read_text())['verified'])
        link = self.root / 'hardlink.img'
        link.hardlink_to(image)
        for same in (image, link):
            with self.assertRaisesRegex(ValueError, 'different inode'):
                mm.validate_backup(manifest, 2, single_read_copy=same)
        for data in (b'z' * 1024, b'a' * 512):
            copy.write_bytes(data)
            with self.assertRaises(ValueError):
                mm.validate_backup(manifest, 2, single_read_copy=copy)
        copy.write_bytes(image.read_bytes())
        metadata['complete'] = False
        manifest.write_text(json.dumps(metadata))
        with self.assertRaises(ValueError):
            mm.validate_backup(manifest, 2, single_read_copy=copy)
        metadata['complete'] = True
        manifest.write_text(json.dumps(metadata))
        image.write_bytes(b'a' * 512)
        with self.assertRaises(ValueError):
            mm.validate_backup(manifest, 2, single_read_copy=copy)

    def test_write_requires_verified_backup_and_exact_source_hash(self):
        medium = Medium(b'a' * 1536)
        original = self.local / 'backup.img'
        mm.backup(medium, original)
        manifest = Path(str(original) + '.json')
        source = self.local / 'new.img'
        source.write_bytes(b'x' * 1024)
        with self.assertRaises(ValueError):
            mm.write_image(medium, source, 1, 'wrong', manifest)
        self.assertEqual(medium.calls, [])
        mm.write_image(medium, source, 1, mm.digest_file(source), manifest)
        self.assertEqual(medium.calls[0], ('arm', 1, 2))
        self.assertEqual([c[1] for c in medium.calls[1:]], [1, 2])
        medium.calls.clear()
        original.write_bytes(b'z' * 1536)
        with self.assertRaises(ValueError):
            mm.write_image(medium, source, 1, mm.digest_file(source), manifest)
        self.assertEqual(medium.calls, [])

    def test_full_restore_from_single_capture_requires_explicit_distinct_copy(self):
        medium = Medium(b'x' * 1536)
        image = self.local / 'original.img'
        image.write_bytes(b'a' * 1536)
        copy = self.root / 'recovery.img'
        copy.write_bytes(image.read_bytes())
        manifest = Path(str(image) + '.json')
        metadata = dict(version=1, image=str(image), total_sectors=3, sector_size=512,
                        complete=True, verified=False, sha256=mm.digest_file(image))
        manifest.write_text(json.dumps(metadata))
        with self.assertRaises(ValueError):
            mm.write_image(medium, image, 0, metadata['sha256'], manifest)
        self.assertEqual(medium.calls, [])
        mm.write_image(medium, image, 0, metadata['sha256'], manifest, single_read_copy=copy)
        self.assertEqual(medium.calls[0], ('arm', 0, 3))
        self.assertEqual([c[1] for c in medium.calls[1:]], [0, 1, 2])
        self.assertFalse(json.loads(manifest.read_text())['verified'])

    def test_uncertain_write_stops_at_first_sector(self):
        medium = Medium(b'a' * 1536)
        original = self.local / 'backup.img'
        mm.backup(medium, original)
        def failed(start, data):
            medium.calls.append(('write', start))
            raise TimeoutError('uncertain')
        medium.write_sector = failed
        source = self.local / 'new.img'
        source.write_bytes(b'x' * 1024)
        with self.assertRaises(TimeoutError):
            mm.write_image(medium, source, 1, mm.digest_file(source), Path(str(original) + '.json'))
        self.assertEqual(medium.calls, [('arm', 1, 2), ('write', 1)])

    def test_public_and_symlink_escaped_paths_rejected(self):
        with self.assertRaises(ValueError):
            mm.private_path(self.root / 'public.img')
        (self.local / 'escape').symlink_to(self.root, target_is_directory=True)
        with self.assertRaises(ValueError):
            mm.private_path(self.local / 'escape' / 'public.img')


if __name__ == '__main__':
    unittest.main()
