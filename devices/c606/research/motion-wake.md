# Motion wake feasibility and listening-sleep observations

## Result

The C606 has a low-power companion that remains responsible for sensors and
main-processor power. The observed stock charging-standby mode stops motion
reports. Leaving the ESP32 in repeated light sleep while the companion is awake
can retain some incoming sensor traffic, but this does not yet implement
motion-triggered startup or recording, and battery savings are unmeasured.

A separate companion application could potentially configure motion wake and
preserve the required power/UART behavior. Acceptance of custom companion code
and recovery from a broken application remain unresolved after the
[guarded marker trial](companion-custom-trial.md). A responding application is
currently required for the known DFU-entry path; see the
[closed-case recovery investigation](companion-recovery-sources.md).

## Stock behavior and hardware uncertainty

Private static analysis of the stock companion identifies six-axis sensor
configuration consistent with the ICM-42607 family, approximately 100 Hz sensor
sampling, and a wake-on-motion configuration. It did not establish a working
stock interrupt path that wakes the companion and starts the ESP32 from off.
The fitted sensor model and the interrupt's electrical wiring were not measured.
These are code-derived leads, not a demonstrated wake-on-motion feature.

On USB, a shutdown request was followed by an ESP32 power restart into charging
standby. Over approximately three minutes, the companion motion report count
remained at one and both motion streams were unavailable. This establishes
absence of continuing motion telemetry in that observed charging-standby state.
It does not measure battery-only off behavior, standby current, or establish
that USB independently powers the ESP32 when the companion is nonfunctional.

The established companion UART uses ESP32 GPIO41/42. Those are not RTC GPIOs
for an ESP32-S3 deep-sleep GPIO wake path. Ordinary GPIO wake in light sleep is
a different mechanism, exercised below. Companion-derived ESP32 power control
and its remaining uncertainty are documented in the recovery investigation.

## Listening-sleep prototype, 2026-11-04

An ESP32-only prototype based on `b94c1f71eab9`, subsequently recorded in
`4ff4bb1de217`, repeatedly entered light sleep with a low-level wake on GPIO41
and a timer. Between sleeps it briefly allowed UART interrupts to run, drained
the receive ring and decoded frames. The stock companion was unchanged. Builds
included the application and diagnostic harness. Raw captures and desk notes
are preserved privately with the stock artifacts.

| Run | Observation interval | RTC sleep fraction | Valid frames | CRC failures | Recovery observation |
| --- | ---: | ---: | ---: | ---: | --- |
| Initial loop without in-loop receive draining | 30 s | 94.6% | Not counted | Not counted | Recovery failed; overflow was suspected, not established. |
| Loop with receive draining | 30 s | 93.7% | 790 | 9 | Recovery completed. |
| Longer loop with receive draining | 300 s | 94.0% | 7,901 | 94 | Recovery completed; zero counted UART errors and no reported crash. |

The long run recorded 281,987,706 microseconds asleep over 300,004,835
microseconds elapsed, with 2,102 GPIO wakes, one timer wake and 2,400 rejected
sleep entries. Valid reception averaged approximately 26 frames per second.
The CRC failure fraction was about 1.2% of valid-plus-CRC-failed frames. Loss
of an initial wake-triggering byte is one plausible explanation; it was not
measured directly. Total frame counts combine report kinds and do not establish
the proportion of retained motion reports. Subsequent ordinary acquisition
showed fresh motion and battery reports and no companion UART/CRC errors.

The prototype used a fixed interval and consumed decoded frames for diagnostics;
it did not route those in-loop samples into automatic recording or wake
classification. Its synthetic TimerWake outcome merely allowed the ordinary
recovery sequence to run, even after repeated GPIO wakes/rejected entries. It
is not the production sleep contract. The PR restores the ordinary bounded
sleep implementation rather than shipping the prototype as POWER SLEEP.
The prototype remains identifiable in commit `4ff4bb1de217` for future work.

## Unresolved work

RTC sleep fraction is not battery-current reduction. The patched HAL retains
clock and memory domains, so generic chip deep-sleep or light-sleep current
figures cannot be applied to this run. No calibrated current measurement,
GNSS power comparison, false-wake rate, motion classifier, automatic ride-start
latency, or outdoor end-to-end motion-start test was performed.

The original feasibility question therefore remains open: choose and measure a
standby design, establish how it obtains reliable motion input, and implement
recording policy only after those mechanisms are demonstrated. These findings
support further investigation; they do not establish an always-on recording
feature or guaranteed companion rollback.
