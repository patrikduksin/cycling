"""A device plugged into a visitor's browser at https://cycling.computer, here as a local port.

    cycling remote <code>                    # bridge until the visitor disconnects
    cycling remote <code> -- <command...>    # run one command against the device, then tear down
    cycling remote reset <code> <kind>       # usbJtagBootloader | hardReset on a running bridge

The bridge (web/agent, run with bun) exports CYCLING_PORT, a pty every
os.open-based tool can use, and CYCLING_ESPTOOL_PORT, an rfc2217:// URL for
esptool. This module owns paths and the token; the bridge owns the protocol
and learns nothing about the repository but these environment variables.
"""

import os
import shutil
import sys

from cycling_devtools.workspace import DEVICE_LOCAL, ROOT

BRIDGE = DEVICE_LOCAL / 'bridge'
TOKEN = BRIDGE / 'token'
AGENT = ROOT / 'web' / 'agent' / 'src' / 'main.ts'


def token(environ):
    if value := environ.get('CYCLING_REMOTE_TOKEN', '').strip():
        return value
    try:
        value = TOKEN.read_text().strip()
    except FileNotFoundError:
        value = ''
    if not value:
        sys.exit(f'no relay token: put the AGENT_TOKEN the relay was deployed with in {TOKEN} (see web/README.md)')
    return value


def environment(environ):
    BRIDGE.mkdir(parents=True, exist_ok=True, mode=0o700)
    BRIDGE.chmod(0o700)
    return environ | {'CYCLING_REMOTE_TOKEN': token(environ), 'CYCLING_REMOTE_DIR': str(BRIDGE)}


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    if not argv or argv[0] in {'-h', '--help'}:
        sys.exit(__doc__)
    env = environment(dict(os.environ))
    bun = shutil.which('bun')
    if bun is None:
        sys.exit('bun is not on PATH: enter the dev shell (nix develop)')
    os.execve(bun, [bun, str(AGENT), *argv], env)


if __name__ == '__main__':
    main()
