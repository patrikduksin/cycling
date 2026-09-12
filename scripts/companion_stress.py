"""Measure companion/input progression through ordinary terminal requests."""
import argparse
import json
from service_stress import run

if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('output')
    p.add_argument('--port', default='/dev/ttyACM0')
    p.add_argument('--seconds', type=float, default=20)
    p.add_argument('--interval', type=float, default=.05)
    p.add_argument('--allow-loss', action='store_true')
    args = p.parse_args()
    print(json.dumps(run(args.port, args.output, args.seconds, 'INPUT', args.interval, args.allow_loss), indent=2))
