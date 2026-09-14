"""Write private #100 scenarios for the existing C606 harness; no device access."""
import argparse
import json
from pathlib import Path

from harness import atomic_json, fields, preflight, private
from usb import ROOT


def scenarios(wifi_profile, original_wifi_connected=True):
    wifi = {'connectivity_restore': {'wifi': {'profile': str(wifi_profile), 'connected': original_wifi_connected}}, 'steps': [
        {'op': 'wifi', 'action': 'scan'},
        {'op': 'wifi', 'action': 'configure', 'profile': str(wifi_profile)},
        {'op': 'wifi', 'action': 'connect'},
        {'op': 'wait', 'command': 'WIFI', 'equals': {'state': '4'}, 'timeout_s': 90},
        {'op': 'wifi', 'action': 'disconnect', 'expect': {'online': 'false'}},
        {'op': 'wifi', 'action': 'configure', 'profile': str(wifi_profile), 'invalid_password': True},
        {'op': 'wifi', 'action': 'connect', 'completion': 'error', 'expect': {'online': 'false'}},
        {'op': 'wifi', 'action': 'configure', 'profile': str(wifi_profile)},
        {'op': 'wifi', 'action': 'connect'},
        {'op': 'wait', 'command': 'WIFI', 'equals': {'state': '4'}, 'timeout_s': 90},
        {'op': 'recover', 'mode': 'restart'},
        {'op': 'wait', 'command': 'WIFI', 'equals': {'state': '4', 'saved': 'true'}, 'timeout_s': 90},
        {'op': 'wifi', 'action': 'forget', 'expect': {'saved': 'false', 'online': 'false'}},
    ]}
    # Preserve the pre-existing compiled authorized SDK peer while returning runtime BLE to echo.
    # The owned laptop fixture runs --profile both so both existing consumers can be checked.
    # Connect includes scan, connection and four bounded GATT phases; allow their full budget.
    ble = {'connectivity_restore': {'ble': {'mode': 'authorized-heart', 'connected': False, 'echo_after': True}}, 'steps': [
        {'op': 'ble', 'action': 'scan'},
        {'op': 'ble', 'action': 'select', 'mode': 'sim-heart'},
        {'op': 'ble', 'action': 'connect', 'timeout_s': 90, 'expect': {'link': 'connected'}},
        {'op': 'command', 'command': 'BLE', 'baseline': 'heart'},
        {'op': 'delay', 'seconds': 3},
        {'op': 'command', 'command': 'BLE', 'progress_from': 'heart', 'counters': ['notifications'], 'unchanged': ['dropped']},
        {'op': 'wait', 'command': 'RIDE SENSORS', 'equals': {'profile': 'heart', 'heart': 'Some(72)', 'invalid': '0'}, 'timeout_s': 10},
        {'op': 'ble', 'action': 'disconnect', 'expect': {'link': 'off'}},
        {'op': 'ble', 'action': 'connect', 'timeout_s': 90, 'expect': {'link': 'connected'}},
        {'op': 'wait', 'command': 'RIDE SENSORS', 'equals': {'profile': 'heart', 'heart': 'Some(72)', 'invalid': '0'}, 'timeout_s': 10},
        {'op': 'ble', 'action': 'select', 'mode': 'sim-csc'},
        {'op': 'ble', 'action': 'connect', 'timeout_s': 90, 'expect': {'link': 'connected', 'profile': '2'}},
        {'op': 'command', 'command': 'BLE', 'baseline': 'cadence'},
        {'op': 'delay', 'seconds': 3},
        {'op': 'command', 'command': 'BLE', 'progress_from': 'cadence', 'counters': ['notifications'], 'unchanged': ['dropped']},
        {'op': 'wait', 'command': 'RIDE SENSORS', 'equals': {'profile': 'cadence', 'cadence': 'Some(600)', 'heart': 'None', 'invalid': '0'}, 'timeout_s': 10},
        {'op': 'recover', 'mode': 'restart'},
        {'op': 'wait', 'command': 'BLE', 'equals': {'link': 'connected', 'saved': 'true', 'profile': '2'}, 'timeout_s': 90},
        {'op': 'wait', 'command': 'RIDE SENSORS', 'equals': {'cadence': 'Some(600)'}, 'timeout_s': 10},
        {'op': 'ble', 'action': 'forget', 'expect': {'saved': 'false', 'link': 'off'}},
        {'op': 'ble', 'action': 'echo'},
        {'op': 'wait', 'command': 'BLE', 'equals': {'link': 'advertising'}, 'timeout_s': 10},
    ]}
    return {'connectivity-wifi': wifi, 'connectivity-ble': ble}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--wifi-profile', default=str(ROOT / '.local/wifi/config.json'))
    parser.add_argument('--baseline', default=str(ROOT / '.local/foundation/original/state.json'))
    parser.add_argument('--output', default=str(ROOT / '.local/foundation/scenarios'))
    args = parser.parse_args()
    baseline = json.loads(private(args.baseline).read_text())
    original_ble = fields(baseline['BLE']['data'])
    if original_ble.get('link') != 'advertising' or fields(baseline['INFO']['data']).get('cycling') != 'false':
        raise ValueError('generator requires the captured original idle base echo composition; supply a deliberate restoration plan for other originals')
    original_wifi = fields(baseline['WIFI']['data'])
    output = private(args.output)
    output.mkdir(parents=True, exist_ok=True, mode=0o700)
    for name, scenario in scenarios(private(args.wifi_profile), original_wifi.get('online') == 'true').items():
        preflight(scenario)
        atomic_json(output / (name + '.json'), scenario)
    print('Wrote connectivity-wifi.json and connectivity-ble.json in the private scenario directory.')


if __name__ == '__main__':
    try:
        main()
    except (ValueError, KeyError, OSError) as error:
        raise SystemExit(f'Scenario generation failed ({type(error).__name__}); inspect private baseline/profile without printing credentials.') from None
