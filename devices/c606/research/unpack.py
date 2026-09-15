#!/usr/bin/env python3
"""Read-only unpacker for the four observed Magene C606 OTA packages.

Standard library only. Produces analysis artifacts, never a flashable repack.
Decoded ELF files are reconstructed for analysis; they are not original build ELFs.
"""
import argparse
import binascii
import collections
import datetime
import functools
import hashlib
import json
import operator
from pathlib import Path
import re
import struct

FIXED_XOR = bytes.fromhex("9a924542")
HEADER_SIZE = 128
MAX_SIZE = 64 * 1024 * 1024

class FormatError(ValueError):
    pass

def require(condition, message):
    if not condition:
        raise FormatError(message)

def sha256(data):
    return hashlib.sha256(data).hexdigest()

def xor(data, key):
    return bytes(v ^ key[i % len(key)] for i, v in enumerate(data))

def cstr(data):
    return data.split(b"\0", 1)[0].decode("ascii", errors="replace")

def decode_package(raw):
    require(HEADER_SIZE + 2 <= len(raw) <= MAX_SIZE, "invalid package size")
    recorded_crc = struct.unpack_from("<H", raw, len(raw) - 2)[0]
    actual_crc = binascii.crc_hqx(raw[:-2], 0)
    require(recorded_crc == actual_crc, "outer CRC-16/XMODEM mismatch")
    outer = xor(raw[:-2], FIXED_XOR)
    header = outer[:HEADER_SIZE]
    require(header[:4] == bytes.fromhex("a55a55aa"), "unexpected package magic")
    words = struct.unpack("<32I", header)
    size = words[5]
    require(size == len(raw) - HEADER_SIZE - 2, "declared payload size mismatch")
    payload_key = header[6:8]
    payload = xor(outer[HEADER_SIZE:], payload_key)
    metadata = {
        "package_bytes": len(raw), "package_sha256": sha256(raw),
        "header_bytes": HEADER_SIZE, "payload_bytes": size,
        "outer_crc16_stored": f"0x{recorded_crc:04x}",
        "outer_crc16_valid": True,
        "fixed_xor_key_hex": FIXED_XOR.hex(),
        "payload_xor_key_hex": payload_key.hex(),
        "unknown_u16_at_0x04": f"0x{struct.unpack_from('<H', header, 4)[0]:04x}",
        "header_words_le": [f"0x{x:08x}" for x in words],
        "payload_sha256": sha256(payload),
        "package_component_name": cstr(header[0x2c:0x4c]),
        "format_text": cstr(header[0x4c:0x58]),
        "hardware_text": cstr(header[0x58:0x68]),
        "package_build_text": cstr(header[0x68:0x80]),
    }
    return header, payload, metadata

def esp_image(payload):
    require(len(payload) >= 24 and payload[0] == 0xe9, "invalid ESP image header")
    count = payload[1]
    require(1 <= count <= 16, "invalid ESP segment count")
    chip = struct.unpack_from("<H", payload, 12)[0]
    require(chip == 9, "only observed ESP32-S3 chip ID 9 is supported")
    entry = struct.unpack_from("<I", payload, 4)[0]
    segments = []
    offset = 24
    checksum = 0xef
    for i in range(count):
        require(offset + 8 <= len(payload), "truncated ESP segment header")
        address, length = struct.unpack_from("<II", payload, offset)
        start, end = offset + 8, offset + 8 + length
        require(end <= len(payload) and address + length <= 2**32, "invalid ESP segment bounds")
        data = payload[start:end]
        checksum = functools.reduce(operator.xor, data, checksum)
        if 0x42000000 <= address < 0x44000000:
            kind, flags = "irom", 5
        elif 0x40370000 <= address < 0x40400000:
            kind, flags = "iram", 5
        elif 0x3c000000 <= address < 0x3e000000:
            kind, flags = "drom", 4
        elif 0x3fc00000 <= address < 0x40000000:
            kind, flags = "dram", 6
        else:
            kind, flags = "unknown", 4
        segments.append({"index": i, "kind": kind, "address": address,
                         "file_offset": start, "size": length, "flags": flags,
                         "sha256": sha256(data)})
        offset = end
    checksum_offset = ((offset + 1 + 15) // 16) * 16 - 1
    require(checksum_offset < len(payload), "missing ESP checksum")
    require(not any(payload[offset:checksum_offset]), "nonzero ESP padding")
    require(payload[checksum_offset] == checksum, "ESP segment XOR checksum mismatch")
    require(payload[23] == 1, "expected appended ESP SHA-256")
    digest_start = checksum_offset + 1
    require(digest_start + 32 <= len(payload), "missing ESP SHA-256")
    digest = hashlib.sha256(payload[:digest_start]).digest()
    require(digest == payload[digest_start:digest_start + 32], "ESP appended SHA-256 mismatch")
    trailing = payload[digest_start + 32:]
    require(any(s["address"] <= entry < s["address"] + s["size"]
                and s["flags"] & 1 for s in segments), "ESP entry is outside code segments")
    app_start = segments[0]["file_offset"]
    desc = payload[app_start:app_start + 256]
    require(len(desc) == 256 and struct.unpack_from("<I", desc)[0] == 0xabcd5432,
            "missing ESP app descriptor")
    app = {"secure_version": struct.unpack_from("<I", desc, 4)[0],
           "version": cstr(desc[16:48]), "project_name": cstr(desc[48:80]),
           "compile_time": cstr(desc[80:96]), "compile_date": cstr(desc[96:112]),
           "idf_version_field": cstr(desc[112:144]), "original_elf_sha256": desc[144:176].hex()}
    return {"architecture": "Xtensa LX7 / ESP32-S3", "elf_machine": 94,
            "entry_point": entry, "chip_id": chip, "segments": segments,
            "esp_checksum_valid": True, "esp_sha256_valid": True,
            "esp_checksum_offset": checksum_offset, "esp_sha256_offset": digest_start,
            "bytes_after_esp_sha256": len(trailing),
            "signature_observation": "No bytes after simple SHA-256; no appended ESP signature block present" if not trailing else "Additional bytes require inspection",
            "app_descriptor": app}

def nordic_image(header, payload):
    require(b"NRF52810_APP" in header, "unsupported non-ESP payload")
    embedded_offset, base = struct.unpack_from("<II", header, 8)
    require(embedded_offset + HEADER_SIZE <= len(payload), "embedded header outside Nordic payload")
    require(payload[embedded_offset:embedded_offset + HEADER_SIZE] == header,
            "Nordic embedded descriptor does not match wrapper header")
    sp, reset = struct.unpack_from("<II", payload)
    require(0x20000000 <= sp <= 0x20006000 and sp % 8 == 0, "Nordic initial SP outside expected 24KB SRAM")
    require(reset & 1 and base <= (reset & ~1) < base + len(payload),
            "Nordic reset vector is not Thumb code within image")
    # Conservatively retain one complete image segment; it contains vectors, code and data.
    segments = [{"index": 0, "kind": "flash", "address": base,
                 "file_offset": 0, "size": len(payload), "flags": 7,
                 "sha256": sha256(payload)}]
    return {"architecture": "ARM Cortex-M4 Thumb / nRF52810", "elf_machine": 40,
            "entry_point": reset, "load_address": base, "initial_stack_pointer": sp,
            "embedded_header_offset": embedded_offset, "embedded_header_matches": True,
            "reset_vector_valid": True, "segments": segments,
            "signature_observation": "Not determined; no Nordic signature-verification claim made"}

def align(value, alignment=16):
    return (value + alignment - 1) // alignment * alignment

def analysis_elf(payload, metadata):
    """Minimal ELF32 with PT_LOAD and named sections. No original symbol/debug info."""
    segments = metadata["segments"]
    blob = bytearray(align(52 + 32 * len(segments)))
    names = bytearray(b"\0")
    sh_names = []
    phdrs, shdrs = [], [(0,) * 10]
    for segment in segments:
        name = f".{segment['kind']}{segment['index']}".encode()
        sh_name = len(names); names.extend(name + b"\0"); sh_names.append(sh_name)
        position = align(len(blob)); blob.extend(b"\0" * (position - len(blob)))
        start, size = segment["file_offset"], segment["size"]
        blob.extend(payload[start:start + size])
        addr, flags = segment["address"], segment["flags"]
        phdrs.append((1, position, addr, addr, size, size, flags, 4))
        section_flags = 2 | (4 if flags & 1 else 0) | (1 if flags & 2 else 0)
        shdrs.append((sh_name, 1, section_flags, addr, position, size, 0, 0, 4, 0))
    names_name = len(names); names.extend(b".shstrtab\0")
    names_offset = len(blob); blob.extend(names)
    shdrs.append((names_name, 3, 0, 0, names_offset, len(names), 0, 0, 1, 0))
    shoff = align(len(blob), 4); blob.extend(b"\0" * (shoff - len(blob)))
    for section in shdrs:
        blob.extend(struct.pack("<10I", *section))
    ident = b"\x7fELF\x01\x01\x01" + b"\0" * 9
    flags = 0x05000200 if metadata["elf_machine"] == 40 else 0
    header = struct.pack("<16sHHIIIIIHHHHHH", ident, 2, metadata["elf_machine"], 1,
                         metadata["entry_point"], 52, shoff, flags, 52, 32,
                         len(phdrs), 40, len(shdrs), len(shdrs) - 1)
    blob[:52] = header
    for i, ph in enumerate(phdrs):
        struct.pack_into("<8I", blob, 52 + 32*i, *ph)
    return bytes(blob)

def strings_with_addresses(payload, segments, minimum=8):
    rows = []
    for match in re.finditer(rb"[ -~]{" + str(minimum).encode() + rb",}", payload):
        offset = match.start(); address = None
        for segment in segments:
            if segment["file_offset"] <= offset < segment["file_offset"] + segment["size"]:
                address = segment["address"] + offset - segment["file_offset"]
                break
        rows.append({"offset": offset, "address": address, "text": match.group().decode("ascii")})
    return rows

def unpack_file(source, output):
    raw = source.read_bytes()
    header, payload, metadata = decode_package(raw)
    metadata.update(esp_image(payload) if payload[0] == 0xe9 else nordic_image(header, payload))
    metadata["source_name"] = source.name
    destination = output / source.stem
    require(source.resolve() != (destination / "firmware.bin").resolve(), "output overlaps input")
    destination.mkdir(parents=True, exist_ok=True)
    (destination / "wrapper-header.bin").write_bytes(header)
    (destination / "firmware.bin").write_bytes(payload)
    elf = analysis_elf(payload, metadata)
    (destination / "analysis.elf").write_bytes(elf)
    metadata["analysis_elf_sha256"] = sha256(elf)
    metadata["analysis_elf_note"] = "Reconstructed memory mapping; not the original build ELF, no symbols"
    segments_dir = destination / "segments"; segments_dir.mkdir(exist_ok=True)
    for segment in metadata["segments"]:
        start, length = segment["file_offset"], segment["size"]
        (segments_dir / f"{segment['index']:02d}_{segment['kind']}_{segment['address']:08x}.bin").write_bytes(payload[start:start+length])
    rows = strings_with_addresses(payload, metadata["segments"])
    metadata["ascii_strings_at_least_8_bytes"] = len(rows)
    (destination / "strings.json").write_text(json.dumps(rows, indent=2))
    (destination / "metadata.json").write_text(json.dumps(metadata, indent=2))
    return metadata

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("inputs", nargs="+", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    results = []
    for source in args.inputs:
        result = unpack_file(source, args.output); results.append(result)
        print(f"{source.name}: {result['architecture']}; payload {result['payload_bytes']:,} bytes; integrity checks passed")
    (args.output / "inventory.json").write_text(json.dumps(results, indent=2))

if __name__ == "__main__":
    main()
