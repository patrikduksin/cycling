import unittest

from regression import go_home


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
    def test_dimmed_diagnostics_wakes_before_returning_home(self):
        device = FakeDevice(['diagnostics', 'device', 'home'], dimmed=True)
        self.assertEqual(go_home(device)['screen'], 'home')
        self.assertEqual(device.buttons, 2)

    def test_home_reached_on_third_allowed_back_is_success(self):
        device = FakeDevice(['depth3', 'depth2', 'depth1', 'home'])
        self.assertEqual(go_home(device)['screen'], 'home')
        self.assertEqual(device.buttons, 3)


if __name__ == '__main__':
    unittest.main()
