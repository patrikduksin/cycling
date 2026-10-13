"""Named harness scenarios. The runner validates every recipe before it runs."""

RECIPES = {
    'input-screen': {
        'steps': [
            {'op': 'command', 'command': 'INPUT'},
            {'op': 'input', 'gesture': 'tap', 'x': 30, 'y': 30},
            {'op': 'input', 'gesture': 'hold', 'x': 40, 'y': 40, 'duration_ms': 500},
            {'op': 'input', 'gesture': 'swipe', 'x': 20, 'y': 20, 'to_x': 100, 'to_y': 120},
            {'op': 'input', 'gesture': 'button', 'button': 0, 'repeat': 2},
            {'op': 'capture', 'count': 1},
            {'op': 'command', 'command': 'INPUT'},
        ]
    },
    'frames': {'steps': [{'op': 'capture', 'count': 4, 'interval_ms': 250}]},
    'recovery': {
        'steps': [
            {'op': 'command', 'command': 'POSITION'},
            {'op': 'recover', 'mode': 'terminal', 'seconds': 2},
            {'op': 'command', 'command': 'POSITION'},
            {'op': 'recover', 'mode': 'restart'},
            {'op': 'command', 'command': 'INPUT'},
        ]
    },
    'shared': {
        'steps': [
            {'op': 'command', 'command': 'INFO'},
            {'op': 'command', 'command': 'SETTINGS'},
            {'op': 'input', 'gesture': 'tap', 'x': 30, 'y': 30},
            {'op': 'capture', 'count': 2, 'interval_ms': 250},
            {'op': 'recover', 'mode': 'restart'},
            {'op': 'command', 'command': 'SETTINGS'},
        ]
    },
    'av': {
        'captures': [
            {'kind': 'camera', 'seconds': 5},
            {'kind': 'microphone', 'seconds': 5, 'fixture': True, 'floor_db': -60, 'margin_db': 8},
        ],
        'steps': [{'op': 'capture', 'count': 2, 'interval_ms': 500}],
    },
}
RECIPES['virtual-foundation'] = {
    'steps': [
        {'op': 'virtual-command', 'command': 'BRIGHTNESS 61'},
        {'op': 'virtual-command', 'command': 'SAVE'},
        {'op': 'recover', 'mode': 'restart'},
        {'op': 'command', 'command': 'SETTINGS', 'expect': {'brightness': '61'}},
        {'op': 'fixture', 'command': 'STORAGE_FAIL 1 12'},
        {'op': 'virtual-command', 'command': 'BRIGHTNESS 42'},
        {'op': 'virtual-command', 'command': 'SAVE', 'status': 'FAILED'},
        {'op': 'recover', 'mode': 'restart'},
        {'op': 'command', 'command': 'SETTINGS', 'expect': {'brightness': '61'}},
        {
            'op': 'fixture',
            'command': 'POSITION NMEA $GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A',
        },
        {'op': 'command', 'command': 'POSITION', 'expect': {'transport': 'receiving'}},
        {'op': 'delay', 'seconds': 4},
        {'op': 'command', 'command': 'POSITION', 'expect': {'transport': 'silent'}},
        {'op': 'fixture', 'command': 'POSITION LOSS'},
        {'op': 'command', 'command': 'POSITION', 'expect': {'transport': 'failed'}},
        {'op': 'fixture', 'command': 'BLE CONNECT'},
        {'op': 'fixture', 'command': 'BLE LOSS'},
        {'op': 'fixture', 'command': 'BLE DISCONNECT'},
    ]
}
RECIPES['virtual-sdk'] = {
    'steps': [
        {'op': 'delay', 'seconds': 3},
        {'op': 'virtual-command', 'command': 'RIDE STATUS', 'expect': {'state': 'ready'}},
        {'op': 'virtual-command', 'command': 'RIDE START', 'status': 'ACCEPTED'},
        {'op': 'delay', 'seconds': 1},
        {'op': 'fixture', 'command': 'BLE CONNECT'},
        {'op': 'fixture', 'command': 'BLE HEART 88'},
        {'op': 'virtual-command', 'command': 'RIDE SENSORS', 'expect': {'heart': 'Some(88)'}},
        {'op': 'delay', 'seconds': 1.2},
        {'op': 'recover', 'mode': 'restart'},
        {'op': 'delay', 'seconds': 5},
        {'op': 'virtual-command', 'command': 'RIDE STATUS', 'expect': {'rides': '1'}},
        {'op': 'virtual-command', 'command': 'EXPORT INFO'},
    ]
}
RECIPES['acceptance'] = {
    'captures': [
        {'kind': 'camera', 'seconds': 10, 'samples_s': [1.5, 2.5, 3.5, 4.5, 9.5]},
        {
            'kind': 'microphone',
            'seconds': 5,
            'fixture': True,
            'floor_db': -60,
            'margin_db': 8,
            'position': 'Laptop microphone and laptop speaker fixture; document unchanged physical placement in evidence.',
        },
    ],
    'steps': [
        {'op': 'delay', 'seconds': 2},
        {'op': 'command', 'command': 'BRIGHTNESS 100'},
        {'op': 'command', 'command': 'DISPLAY f800'},
        {'op': 'delay', 'seconds': 1},
        {'op': 'command', 'command': 'DISPLAY 07e0'},
        {'op': 'delay', 'seconds': 1},
        {'op': 'command', 'command': 'BRIGHTNESS 20'},
        {'op': 'delay', 'seconds': 1},
        {'op': 'command', 'command': 'BRIGHTNESS 100'},
        {'op': 'command', 'command': 'POSITION', 'baseline': 'gnss'},
        {'op': 'command', 'command': 'INPUT', 'baseline': 'companion'},
        {'op': 'command', 'command': 'WIFI'},
        {'op': 'command', 'command': 'BLE'},
        {'op': 'input', 'gesture': 'tap', 'x': 30, 'y': 30},
        {'op': 'input', 'gesture': 'hold', 'x': 40, 'y': 40, 'duration_ms': 500},
        {'op': 'input', 'gesture': 'drag', 'x': 20, 'y': 20, 'to_x': 100, 'to_y': 120},
        {'op': 'input', 'gesture': 'button', 'button': 0, 'repeat': 2},
        {'op': 'capture', 'count': 1},
        {'op': 'capture', 'count': 4, 'interval_ms': 250},
        {
            'op': 'command',
            'command': 'POSITION',
            'progress_from': 'gnss',
            'counters': ['bytes', 'valid'],
            'unchanged': ['checksum_errors', 'parse_errors', 'dma_losses', 'line_overflows', 'uart_errors'],
        },
        {
            'op': 'command',
            'command': 'INPUT',
            'progress_from': 'companion',
            'counters': ['companion_valid'],
            'unchanged': ['bad_crc', 'uart_errors', 'touch_errors', 'input_lost'],
        },
        {'op': 'recover', 'mode': 'terminal', 'seconds': 1},
        {'op': 'recover', 'mode': 'usb-reset', 'seconds': 1},
        {'op': 'recover', 'mode': 'restart'},
    ],
}
