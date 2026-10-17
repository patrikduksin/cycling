# VANA UX flows

This is the source of truth for how a rider moves through VANA. [UI](ui.md)
owns how each screen looks, and [ride recording](ride-recording.md) owns what
is stored. The simulator scenario
`the_ride_lifecycle_walks_from_ready_to_the_summary` walks this flow and keeps
a screenshot of each step.

## Words

Copy is short, plain and the same everywhere:

- Say what is true and what to do, never how it works inside: `NO SENSORS`
  and `TAP HERE TO PAIR ONE`, not device numbers or scan states.
- Use one word per thing: a *ride* (not an activity or session), *pause*,
  *resume*, *lap*, *end ride*, *saved*.
- A headline is the state, and a grey line under it says what to do, only
  when the rider needs to do something.
- No button labels. A screen that asks a question points at the answer with a
  [button cue](ui.md#button-cues).

## The ride

```text
Home ── RIDE ──> Ready ── ACT or roll off ──> Ride pages (recording)
                  │ BACK: Home                    │ ACT          ^
                                                  v              │ ACT (resume)
                                            Paused screen ───────┘
                                              │ MOVE: ride pages, and back
                                              │ BACK
                                              v
                                          END RIDE? ── BACK ──> Paused screen
                                              │ ACT
                                              v
                                         Saving ──> Ride saved summary (3 pages)
                                                       │ ACT (Done) or BACK
                                                       v
                                                      Home
```

### Ready

Home's RIDE card opens Ready. It answers "can I go?" in four lines, each a
coloured dot, a headline and, when needed, what to do:

| Line | Good (green) | Waiting or worth knowing (amber) | Problem (red) | Not in use (grey) |
| --- | --- | --- | --- | --- |
| GPS | GPS LOCKED | FINDING GPS, you can start anyway | | NO GPS, records without a map |
| Sensors | n SENSORS LIVE, with their short names | SENSORS WAKING UP, naming the missing ones; NO SENSORS, tap here to pair one | | |
| Battery | BATTERY n% | under 30%: charge after the ride | under 15%: charge before riding | |
| Storage | ROOM FOR n H | ROOM FOR n MIN or UNDER 1 MIN OF ROOM: export old rides; on internal memory | STORAGE FULL, STORAGE PROBLEM | |

The headline sums them up: READY when every line is green or grey, ALMOST
READY when something is amber, and NOT READY (red) when storage cannot record.
Only storage stops a ride; everything else is advice, so a rider is never
held at a green light by a sensor. Under the headline, the ride type reads
FREE RIDE, or the chosen workout's name.

ACT starts the ride (the green play cue points at it); there is no
countdown, because a countdown delays the start at a light, and rolling off
starts the ride from Ready anyway (auto-start). BACK returns Home. Tapping
the sensors line opens Sensors and starts a search. A ride that started a
moment before the press (rolling off) is only shown, never paused, by ACT.

### Recording

Starting switches straight to the ride pages, shows a RECORDING toast and
plays the start chime. On the ride pages ACT pauses, MOVE turns the page and
BACK takes a lap (with a lap toast and the lap chime). The ride pages never
show cues: the buttons keep fixed roles.

### Paused

A pause, by ACT or by auto-pause, fills the screen: two amber bars, PAUSED or
AUTO PAUSED, how long the pause has lasted, the ride time and the distance.
The pause chime plays once. An automatic pause adds RIDE ON TO RESUME, and
rolling on resumes it with AUTO RESUME. ACT resumes (RESUMED toast and the
start chime).

MOVE shows the ride pages from the page the rider was on, with PAUSED in the
status bar; paging past the last page comes back to the paused screen. Swipes
do the same.

### End ride

BACK while paused asks END RIDE? with the ride time and distance. This
follows the same pattern as TURN OFF: BACK asks, ACT answers yes, BACK answers
no. The question never times out. Answering yes finishes the ride, shows
SAVING (keep VANA on) while the last samples are written, then the summary.

### Ride saved

The summary opens with a green check and RIDE SAVED, and pages through:

1. Time, distance, average speed, ascent, average power and average heart
   rate.
2. Normalized, average and maximum power, work in kJ, average and maximum
   heart rate, and power and heart-rate zone bars.
3. Best power over 5 s, 1, 5 and 20 min as bars against the best of them,
   then climbs and laps.

MOVE or a swipe turns the page; ACT (Done) or BACK goes Home. If the last
write failed, page 1 says NOT ALL SAVED in amber instead of RIDE SAVED.

The 5 s and 1 min bests are exact. The 5 and 20 min bests use 10 s buckets, so
they can read slightly low but never high.

## Other routes

- **Phone.** `Page::Phone` is a placeholder for the phone companion: PHONE and
  "SEND RIDES TO YOUR PHONE"; BACK goes Home. That workstream owns its Home
  entry.
