"""Export stopped on-device ANT/GPS captures, preserving the complete occupied prefix."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import struct
import zlib

from usb import UsbConnection

SLOT_SIZE = 256
MAX_SLOTS = 4096
LINKS = ('idle', 'connecting', 'connected', 'disconnecting', 'disconnected', 'timed_out', 'transport_lost')


def command(connection, text):
    reply = connection.terminal_command(f'RADAR LOG {text}')
    if reply['status'] != 'OK':
        raise RuntimeError(f'capture export rejected: {reply["status"]}')
    return reply['data'].split()


def info(connection):
    fields = command(connection, 'INFO')
    if len(fields) != 5 or fields[:3] != ['INFO', '1', '256']:
        raise ValueError('unsupported capture export info')
    upper = int(fields[3])
    if not 0 <= upper <= MAX_SLOTS:
        raise ValueError('invalid capture prefix bound')
    return {'upper': upper, 'status': fields[4]}


def decode_slot(data):
    if len(data) != SLOT_SIZE:
        raise ValueError('short slot')
    if data[:4] != b'ANT1':
        return None
    if data[4] != 1 or data[7] != 0 or data[252:] != b'TMOC':
        raise ValueError('unsupported or uncommitted ANT slot')
    if zlib.crc32(data[:248]) != int.from_bytes(data[248:252], 'little'):
        raise ValueError('ANT slot CRC mismatch')
    kind, count = data[5:7]
    if kind not in (1, 2, 3, 4, 5, 6) or (kind == 1 and not 1 <= count <= 8) or (kind in (4, 5, 6) and count != 1) or (kind in (2, 3) and count != 0):
        raise ValueError('invalid ANT record kind/count')
    record = dict(kind={1: 'packets', 2: 'stopped', 3: 'full', 4: 'link', 5: 'link', 6: 'position'}[kind],
                  capture_id=int.from_bytes(data[8:12], 'little'),
                  sequence=int.from_bytes(data[12:16], 'little'),
                  prepared_ms=int.from_bytes(data[16:24], 'little'))
    if kind == 1:
        records = []
        for i in range(count):
            device_type, transmission, number, received, generation, loss, payload = struct.unpack_from('<BBHQII8s', data, 24 + 28 * i)
            records.append(dict(device_type=device_type, transmission_type=transmission,
                                device_number=number, received_ms=received, generation=generation,
                                transport_loss_count=loss, payload_hex=payload.hex()))
        record['packets'] = records
    elif kind in (4, 5):
        state_offset = 24 if kind == 4 else 25
        if data[state_offset] >= len(LINKS) or any(data[state_offset + 1:28]):
            raise ValueError('invalid link record')
        record.update(link=LINKS[data[state_offset]], received_ms=int.from_bytes(data[28:36], 'little'),
                      generation=int.from_bytes(data[36:40], 'little'),
                      dropped_packets=int.from_bytes(data[40:44], 'little'),
                      dropped_links=int.from_bytes(data[44:48], 'little'))
        if kind == 5:
            record['device_type'] = data[24]
    elif kind == 6:
        flags = data[24]
        now, observed, latitude, longitude, dropped = struct.unpack_from('<QQiiI', data, 28)
        if (flags & ~3 or any(data[25:28])
                or (not flags & 1 and observed != 0)
                or (not flags & 2 and (latitude != 0 or longitude != 0))):
            raise ValueError('invalid position record')
        record.update(sample_ms=now, observed_ms=observed if flags & 1 else None,
                      fix_valid=bool(flags & 2),
                      latitude_e7=latitude if flags & 2 else None,
                      longitude_e7=longitude if flags & 2 else None,
                      source_age_ms=max(0, now - observed) if flags & 1 else None,
                      dropped_positions=dropped)
    else:
        record.update(dropped_packets=int.from_bytes(data[24:28], 'little'),
                      dropped_links=int.from_bytes(data[28:32], 'little'))
        if data[32:36] == b'GPS1':
            record['dropped_positions'] = int.from_bytes(data[36:40], 'little')
    return record


def export(port, output):
    output.mkdir(parents=True, exist_ok=False, mode=0o700)
    partial = output / 'prefix.partial'
    records, invalid = [], []
    with UsbConnection(port, output / 'usb.log') as connection:
        before = info(connection)
        with partial.open('wb') as raw:
            for index in range(before['upper']):
                fields = command(connection, f'READ {index}')
                if len(fields) != 4 or fields[0] != 'SLOT' or int(fields[1]) != index:
                    raise ValueError('wrong capture slot reply')
                data = bytes.fromhex(fields[3])
                if len(data) != SLOT_SIZE or zlib.crc32(data) != int(fields[2], 16):
                    raise ValueError('corrupt capture transport')
                raw.write(data)
                try:
                    record = decode_slot(data)
                except ValueError as error:
                    invalid.append({'slot': index, 'error': str(error)})
                    continue
                if record is not None:
                    record['slot'] = index
                    records.append(record)
            raw.flush()
            os.fsync(raw.fileno())
        if info(connection) != before:
            raise ValueError('capture prefix changed during export')
    raw_path = output / 'prefix.bin'
    partial.rename(raw_path)
    sequences = {}
    gaps = []
    for record in records:
        capture = record['capture_id']
        expected = sequences.get(capture, 0)
        if record['sequence'] != expected or capture > record['slot']:
            gaps.append({'slot': record['slot'], 'capture_id': capture, 'expected_sequence': expected})
        sequences[capture] = record['sequence'] + 1
    manifest = dict(version=1, slot_size=SLOT_SIZE, **before,
                    sha256=hashlib.sha256(raw_path.read_bytes()).hexdigest(),
                    positions=sum(r['kind'] == 'position' for r in records),
                    ant_records=len(records), packets=sum(len(r.get('packets', [])) for r in records),
                    invalid_ant_slots=invalid, sequence_gaps=gaps)
    (output / 'records.json').write_text(json.dumps(records, indent=2))
    (output / 'manifest.json').write_text(json.dumps(manifest, indent=2))
    print(f'Exported {manifest["packets"]} ANT packets and {manifest["positions"]} GPS samples to {output}; invalid ANT slots={len(invalid)}, sequence gaps={len(gaps)}')
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--port', default=os.environ.get('CYCLING_PORT', '/dev/ttyACM0'))
    args = parser.parse_args()
    export(args.port, args.output)


if __name__ == '__main__':
    main()
