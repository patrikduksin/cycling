"""Bounds-checked little-endian integer fields shared by binary decoders."""

import struct

_U16 = struct.Struct('<H')
_U32 = struct.Struct('<I')
_U64 = struct.Struct('<Q')
_I16 = struct.Struct('<h')
_I32 = struct.Struct('<i')


def _unpack(layout, data, offset):
    if offset < 0 or offset + layout.size > len(data):
        raise ValueError('truncated little-endian field')
    return layout.unpack_from(data, offset)[0]


def u16(data, offset):
    return _unpack(_U16, data, offset)


def u32(data, offset):
    return _unpack(_U32, data, offset)


def u64(data, offset):
    return _unpack(_U64, data, offset)


def i16(data, offset):
    return _unpack(_I16, data, offset)


def i32(data, offset):
    return _unpack(_I32, data, offset)
