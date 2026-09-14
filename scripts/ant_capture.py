"""Export stopped ANT/GPS/environment captures, preserving the occupied prefix.

Morning SDK capture needs no ANT peer: send FOUNDATION LOG START 300, then poll
FOUNDATION LOG STATUS until Recording and saved_environment/saved_positions
advance. ACCEPTED and Scanning do not establish durable capture. Status reports
remaining_slots, required_slots and the requested/remaining seconds. The explicit
300..600 second deadline starts after scanning and stops automatically. Full/Error
cannot start a reliable session.
After the physical session send FOUNDATION LOG STOP and wait for Stopped.
Export to a new private directory with:
    mise run ant-export -- .local/foundation-morning --domain FOUNDATION
After restart, let the ride scanner finish before requesting the export.
The capture never erases or overwrites occupied slots. No automatic capture
resumes after restart. Environmental values absent on the wire remain missing;
raw motion is unscaled, battery millivolts are not independently calibrated.
Sampled flag 1 means environment/GPS/latest ANT per selected radar/HR/power type every 2000 ms
and changed link snapshots at most every 10000 ms. Intermediate link transitions
are omitted without a counter. SMP1 counters track deliberate ANT packet omissions
separately from drops. Every record retains this mode even without a terminal.
"""
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


def command(connection, text, domain='RADAR'):
    reply = connection.terminal_command(f'{domain} LOG {text}')
    if reply['status'] != 'OK':
        raise RuntimeError(f'capture export rejected: {reply["status"]}')
    return reply['data'].split()


def info(connection, domain='RADAR'):
    fields = command(connection, 'INFO', domain)
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
    if data[4] != 1 or data[7] not in (0, 1) or data[252:] != b'TMOC':
        raise ValueError('unsupported or uncommitted ANT slot')
    if zlib.crc32(data[:248]) != int.from_bytes(data[248:252], 'little'):
        raise ValueError('ANT slot CRC mismatch')
    kind, count = data[5:7]
    if kind not in (1, 2, 3, 4, 5, 6, 7) or (kind == 1 and not 1 <= count <= 8) or (kind in (4, 5, 6, 7) and count != 1) or (kind in (2, 3) and count != 0):
        raise ValueError('invalid ANT record kind/count')
    record = dict(kind={1: 'packets', 2: 'stopped', 3: 'full', 4: 'link', 5: 'link', 6: 'position', 7: 'environment'}[kind],
                  capture_id=int.from_bytes(data[8:12], 'little'),
                  sequence=int.from_bytes(data[12:16], 'little'),
                  prepared_ms=int.from_bytes(data[16:24], 'little'))
    if data[7] == 1:
        record.update(capture_mode="sampled", sample_interval_ms=2000, link_interval_ms=10000)
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
    elif kind == 7:
        flags = data[24]
        now, losses, invalid, dropped = struct.unpack_from('<QIII', data, 28)
        if (flags & ~31 or any(data[25:28]) or any(data[62:64])
                or any(data[78:80]) or any(data[94:96]) or data[105]
                or any(data[117:120])):
            raise ValueError('invalid environmental flags/reserved bytes')
        tail = 128
        if data[7] == 1 and data[128:132] == b'SMP1':
            record['sampled_out_packets'] = int.from_bytes(data[132:136], 'little')
            tail = 136
        if data[tail:248] != b'\xff' * (248 - tail):
            raise ValueError('invalid environmental flags/reserved bytes')
        blocks = [(1, 48, 62), (2, 64, 78), (4, 80, 94), (8, 96, 108), (16, 108, 117)]
        for mask, begin, end in blocks:
            if not flags & mask and any(data[begin:end]):
                raise ValueError('absent environmental source has data')
            if flags & mask and int.from_bytes(data[begin:begin + 8], 'little') > now:
                raise ValueError('environmental source timestamp exceeds sample')
        def source(timestamp, **values):
            return dict(received_ms=timestamp, source_age_ms=now - timestamp, **values)
        observed, pressure, temperature = struct.unpack_from('<QIh', data, 48)
        if flags & 1 and (not 1_000_000 <= pressure <= 13_000_000 or not -6000 <= temperature <= 10000):
            raise ValueError('invalid environmental pressure')
        record.update(sample_ms=now, sensor_losses=losses, invalid_sensor_reports=invalid,
                      dropped_environment=dropped,
                      pressure=source(observed, pressure_centi_pa=pressure, temperature_centi_c=temperature) if flags & 1 else None)
        for index, at in enumerate((64, 80)):
            observed, x, y, z = struct.unpack_from('<Qhhh', data, at)
            record[f'motion_{index + 1}'] = source(observed, raw_axes=[x, y, z]) if flags & (2 << index) else None
        observed = int.from_bytes(data[96:104], 'little')
        percent, millivolts = data[104], int.from_bytes(data[106:108], 'little')
        if flags & 8 and (percent > 100 or not 2000 <= millivolts <= 5000):
            raise ValueError('invalid environmental battery')
        record['battery'] = source(observed, percent=percent, interpreted_millivolts=millivolts) if flags & 8 else None
        observed = int.from_bytes(data[108:116], 'little')
        record['power'] = source(observed, raw_status=data[116]) if flags & 16 else None
        record['uart_errors'], record['bad_crc'] = struct.unpack_from('<II', data, 120)
    else:
        record.update(dropped_packets=int.from_bytes(data[24:28], 'little'),
                      dropped_links=int.from_bytes(data[28:32], 'little'))
        if data[32:36] == b'GPS1':
            record['dropped_positions'] = int.from_bytes(data[36:40], 'little')
        if data[40:44] == b'ENV1':
            record['dropped_environment'] = int.from_bytes(data[44:48], 'little')
        if data[7] == 1 and data[48:52] == b'SMP1':
            record['sampled_out_packets'] = int.from_bytes(data[52:56], 'little')
    return record


def export(port, output, domain='RADAR', start_slot=0):
    if domain not in ('RADAR', 'FOUNDATION'):
        raise ValueError('unsupported capture domain')
    if type(start_slot) is not int or not 0 <= start_slot <= MAX_SLOTS:
        raise ValueError('invalid capture range lower bound')
    output.mkdir(parents=True, exist_ok=False, mode=0o700)
    artifact = 'range' if start_slot else 'prefix'
    partial = output / f'{artifact}.partial'
    records, invalid = [], []
    with UsbConnection(port, output / 'usb.log') as connection:
        before = info(connection, domain)
        if start_slot > before['upper']:
            raise ValueError('capture range starts beyond occupied prefix')
        with partial.open('wb') as raw:
            for index in range(start_slot, before['upper']):
                fields = command(connection, f'READ {index}', domain)
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
        if info(connection, domain) != before:
            raise ValueError('capture prefix changed during export')
    raw_path = output / f'{artifact}.bin'
    partial.rename(raw_path)
    sequences = {}
    gaps = []
    for record in records:
        capture = record['capture_id']
        expected = sequences.get(capture, 0)
        if record['sequence'] != expected or capture > record['slot']:
            gaps.append({'slot': record['slot'], 'capture_id': capture, 'expected_sequence': expected})
        sequences[capture] = record['sequence'] + 1
    manifest = dict(version=1, slot_size=SLOT_SIZE, lower=start_slot, **before,
                    sha256=hashlib.sha256(raw_path.read_bytes()).hexdigest(),
                    positions=sum(r['kind'] == 'position' for r in records),
                    environmental_samples=sum(r['kind'] == 'environment' for r in records),
                    ant_records=len(records), packets=sum(len(r.get('packets', [])) for r in records),
                    invalid_ant_slots=invalid, sequence_gaps=gaps)
    (output / 'records.json').write_text(json.dumps(records, indent=2))
    (output / 'manifest.json').write_text(json.dumps(manifest, indent=2))
    print(f'Exported {manifest["packets"]} ANT packets and {manifest["positions"]} GPS samples plus {manifest["environmental_samples"]} environmental samples to {output}; invalid ANT slots={len(invalid)}, sequence gaps={len(gaps)}')
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--port', default=os.environ.get('CYCLING_PORT', '/dev/ttyACM0'))
    parser.add_argument('--domain', choices=('RADAR', 'FOUNDATION'), default='RADAR')
    parser.add_argument('--start-slot', type=int, default=0, help='export from this absolute slot through the occupied upper bound; default preserves the full prefix')
    args = parser.parse_args()
    export(args.port, args.output, args.domain, args.start_slot)


if __name__ == '__main__':
    main()
