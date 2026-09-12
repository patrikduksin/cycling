"""Measure GNSS progression through ordinary terminal requests."""
import argparse
import json
from service_stress import run

if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('directory')
    p.add_argument('--port', default='/dev/ttyACM0')
    p.add_argument('--seconds', type=float, default=20)
    p.add_argument('--require-no-loss', action='store_true', help='Kept for existing invocations; no loss is the default')
    p.add_argument('--allow-loss', action='store_true')
    args = p.parse_args()
    print(json.dumps(run(args.port, args.directory, args.seconds, 'POSITION', allow_loss=args.allow_loss), indent=2))
