import unittest

from regression import go_home, scene_change, wait_closed_recorder


class FakeDevice:
    def __init__(self, screens, *, dimmed=False):
        self.screens = list(screens)
        self.dimmed = dimmed
        self.buttons = 0

    def command(self, command):
        if command == 'STATE':
            return {'screen': self.screens[0], 'dimmed': self.dimmed,
                    'x': -1, 'y': -1}
        if command == 'RELEASE':
            self.dimmed = False
        elif command == 'BUTTON 0 1':
            self.buttons += 1
            self.screens.pop(0)
        return {'screen': self.screens[0], 'dimmed': self.dimmed,
                'x': -1, 'y': -1}

    def expect(self, wanted):
        state = self.command('STATE')
        for key, value in wanted.items():
            if state[key] != value:
                raise AssertionError(f'{key}: {state[key]} != {value}')
        return state


class NavigationSetupTests(unittest.TestCase):
    def test_recorder_scan_is_bounded_before_regression_baseline(self):
        class RecorderDevice:
            def __init__(self):
                self.states = ['scanning', 'scanning', 'ready']
                self.waits = 0

            def command(self, command):
                return {'ride_recording': self.states.pop(0)}

            def wait(self, seconds):
                self.waits += 1

        device = RecorderDevice()
        self.assertEqual(wait_closed_recorder(device)['ride_recording'], 'ready')
        self.assertEqual(device.waits, 2)

    def test_dimmed_diagnostics_wakes_before_returning_home(self):
        device = FakeDevice(['diagnostics', 'device', 'home'], dimmed=True)
        self.assertEqual(go_home(device)['screen'], 'home')
        self.assertEqual(device.buttons, 2)

    def test_home_reached_on_third_allowed_back_is_success(self):
        device = FakeDevice(['depth3', 'depth2', 'depth1', 'home'])
        self.assertEqual(go_home(device)['screen'], 'home')
        self.assertEqual(device.buttons, 3)

    def test_ready_minute_one_starts_before_changing_page(self):
        class SceneDevice:
            def __init__(self):
                self.state = {'screen': 'home', 'ride_phase': 'ready', 'ride_page': 0,
                              'dimmed': False, 'x': -1, 'y': -1}
                self.commands = []

            def command(self, command):
                self.commands.append(command)
                if command == 'BUTTON 2 1':
                    self.state['ride_phase'] = 'running'
                elif command == 'BUTTON 1 1':
                    self.state['ride_page'] ^= 1
                return dict(self.state)

            def tap(self, x, y):
                self.state['screen'] = 'ride'

            def expect(self, wanted):
                return self.command('STATE')

        device = SceneDevice()
        scene_change(device, 1)
        self.assertEqual(device.commands[-2:], ['BUTTON 2 1', 'BUTTON 1 1'])
        self.assertEqual(device.state['ride_page'], 1)


if __name__ == '__main__':
    unittest.main()
