# 2. Connection capacity

## Result

**Four is our application limit, not the capacity configured by the examined
companion firmware.** Both N22 releases request 12 ANT channels. N22 1.902 has
ten distinct sensor-type slots plus two discovery slots. Ten simultaneously
working sensors have not been demonstrated.

## Evidence

The radio enable routine at `0x1e920` passes the configuration at `0x241e8` to
SVC `0xfa`. Its first byte requests 12 channels. The routine's own diagnostic
identifies the call as `sd_ant_enable`. N22 1.628 does the same at `0x1e810`, using
configuration `0x240d8`. Nordic documents the first configuration field as the
[total requested channel count](https://docs.nordicsemi.com/r/bundle/s212_v6.1.1_api/page/group_ant_interface.html?contentId=VqX3C_MEycaZNOyjaldT3Q).

The fixed lookup at `0x1529c` maps types as follows:

| Type | Profile/category | Channel |
| --- | --- | --- |
| 40 | Radar | 1 |
| 120 | Heart rate | 2 |
| 11 | Bicycle power | 3 |
| 122 | Cadence | 4 |
| 123 | Speed | 5 |
| 121 | Combined speed/cadence | 6 |
| 34 | Shifting path labeled SHFT | 7 |
| 17 | Fitness equipment | 8 |
| 128 | Path labeled Di2 | 9 |
| 35 | Bicycle lights | 11 |

Channels 0 and 10 belong to discovery on separate ANT networks. Channel 0 uses
network index 0; channel 10 uses index 1 for the Di2 discovery path. Both use
2457 MHz. A general scan alternates between them. They are reserved search
channels, not two spare sensor connections or two extra radio chips. See the
[scan report](04-scanning.md) for the configuration and switching evidence.

Initialization at `0x1bb94` calls the
discovery and profile initialization routines. Type-specific connect functions
use those fixed slots; there is no four-active-sensor check in the examined
connect dispatcher at `0x13114`.

## Remaining uncertainty and decision

Allocation and code paths establish intended capacity, not scheduling success
under every combination. Several initialization results are not enforced, and
live radio timing, profile compatibility and UART load still need measurement.
The current application's four-sensor ride remains the demonstrated baseline.

**A staged increase beyond four is justified for the next experiment.** Start
with an additional different type, such as cadence alongside speed, HR, power
and radar. Verify all channels continue receiving before increasing again.
Allowlisting known bridge types would also make capacity failures clearer than
accepting arbitrary type numbers. Ten is a research target, not a product promise.
