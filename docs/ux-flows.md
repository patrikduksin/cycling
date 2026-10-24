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
                                         Saving ──> Ride saved summary (3 or 4 pages, one more with an FTP offer)
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

### Map

The ride page before BACK TO START, MAP, draws where the ride has been: its
own GPS track as a white line, a green dot where it began, and the rider in
blue. Riding, the track is turned so the way the rider is heading is up and
the rider is an arrowhead; standing still (after about 15 s without moving,
or while paused) it is north up, the rider a dot, and the page says N UP. The
page fits the whole track and the rider to the screen, zooming out in steps
and never closer than 500 m across; it holds still until the track nears the
edge. A bar at the bottom shows the scale (150 M, 1.5 KM) and the ride's distance sits beside
it. Following the track back, the way home is green and the part already left
behind grey, with FOLLOWING in the header. Without a fix the page says NO
GPS YET and MAP STARTS WITH GPS; once a fix is lost the track stays where it
was, with NO GPS in amber. A device with no memory for the track says NO TRACK.

The map draws the same breadcrumb as BACK TO START (below), so a very long
ride shows its bends thinned and a far-off start is a small dot.

### Back to start

The page after MAP, BACK TO START, points the way to where the ride began.
The arrow points relative to the way the rider rides, so straight up is
straight on; standing still, it points north up and says N UP. DIRECT KM is
the distance straight to the start, TRACK KM the distance back the way the
ride came. MOVE reaches it last; a swipe back from the first page reaches it
at once.

Riding back along the ride's own track, against the way it was ridden,
follows it by itself: FOLLOWING TRACK, a rising cue, and a green arrow that
points along the track a little way ahead. More than 50 m off it, OFF TRACK
shows on any page with a falling cue, and again each minute; within 25 m,
BACK ON TRACK. 500 m off, following stops (STOPPED FOLLOWING): the rider chose
another way, and the track still leads home from where they left it. At the
start, BACK AT START. The page's pad (FOLLOW TRACK BACK, STOP FOLLOWING) does
the same sooner; the buttons keep their ride roles. Laps of a loop the same
way round never follow by themselves.

Under the pad a grey line says when the light ends, SUNSET 19:42 in local
time, and DARK once the sun is down. It shows only with a known position and
time, and never in polar day.

The track is the ride's GPS breadcrumb, thinned to its bends, up to 4,096
points in PSRAM. Each ride starts a new one; a restart loses it.

### Sunset

Dusk catches riders out, so VANA works out the sunset (the sun's centre at
-0.833 degrees) from the last fixed position and the GPS or network time, at
most once a minute. While recording, with Settings ALERTS SUNSET on (the
default, and the default for settings saved before it existed), an amber
toast and the attention chime say SUNSET IN 30 MIN, once per ride, when
30 to 20 minutes remain, and SUNSET - LIGHTS ON once when the sun sets. The
alerts follow the SOUNDS setting for the chime and show on the ride pages;
one due while another page is up waits for the rider's return. They need a
known position and time. A ride that starts after sunset says nothing: the
rider can see it is dark. A ride that starts less than 20 minutes before
sunset skips the warning and gives only LIGHTS ON. A ride recovered after a restart, already more than two
minutes old, never repeats the warning either. A pause across the
30-minute mark gives the warning on resume, if 20 minutes or more remain.
Polar day and night have no sunset and no alert.

### Paused

A pause, by ACT or by auto-pause, fills the screen: two amber bars, PAUSED or
AUTO PAUSED, how long the pause has lasted, the ride time and the distance.
The pause chime plays once. An automatic pause adds RIDE ON TO RESUME, and
rolling on resumes it with AUTO RESUME. ACT resumes (RESUMED toast and the
start chime). Riding on after a pause by ACT never resumes the ride, but VANA
plays a rising three-note reminder once the bike has held start speed (6 km/h
on a wheel sensor, 10 km/h on GPS) for a few seconds, then at most once every 30 s
of that pause while the rider keeps riding, however often they stop and go. It
stops when the rider presses ACT. It sounds on any page, follows the SOUNDS setting
and plays with AUTO PAUSE off too, since every pause is then the rider's.

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
2. Normalized and maximum power, work in kJ, KCAL (the work again, as
   calories: 24 % efficient muscle makes kJ and kcal nearly equal; none
   without a power meter), average and maximum heart rate, power and
   heart-rate zone bars, and HR RECOVERY: the fall in heart rate over the
   minute after the ride stopped recording. A ride is paused before it ends,
   so it counts from that pause, with MEASURING and a countdown until the
   minute is up, then the fall (-28 BPM). A rider who waits more than five
   seconds past the minute before ending the ride, a sensor that is silent at
   either moment (ANT+ or BLE, ride open or not), or leaving the summary
   before the minute is up shows nothing. It is not saved with the ride.
3. Elevation, only for a ride that recorded altitude: ASCENT M, DESCENT M,
   MIN M and MAX M in amber over a filled chart of altitude across the whole
   distance, each stretch in its climb steepness colour. A ride without
   altitude has no such page.
4. Best power over 5 s, 1, 5 and 20 min as bars against the best of them,
   then climbs and laps. Without an elevation page this is page 3.

MOVE or a swipe turns the page, wrapping, and the page dots count three or four; ACT (Done) or BACK goes Home. If the last
write failed, page 1 says NOT ALL SAVED in amber instead of RIDE SAVED.

A ride recorded live that sets a personal best (see Bests below) opens its
summary with one toast and the best chime: `BEST 5 MIN 312 W` for one,
`3 NEW BESTS` for more. Each new best says BEST in its caption and takes its
metric colour.

**FTP offer.** When the ride's new 20 min best times 0.95 is at least 3% above
the stored FTP, the summary gets a fourth, last page: `FTP 245 W?`, over the
20 min best it comes from and `NOW 215 W`. It is a page, not a sheet, so the
other three pages keep their buttons: ACT and BACK still go Home. On the
offer, ACT (check cue) takes it (`FTP 245 W SAVED`) and BACK (back cue) keeps
the FTP in force (`FTP KEPT 215 W`); both then go Home. Pressing Done on an
earlier page declines it, and the offer is never shown again for that ride.
The four page dots show that it is there. It is reached only going forward
(MOVE from the last page): a swipe back from the first page lands on page 3,
where ACT is Done as everywhere else. It is dropped when the suggestion no
longer beats the FTP in force by 3%. It is not offered for a saved ride read
back from the list.

The 5 s and 1 min bests are exact. The 5 and 20 min bests use 10 s buckets, so
they can read slightly low but never high.

## Saved rides

```text
Home ── RIDES ──> Rides (newest first) ── ACT or tap ──> Saved ride (3 or 4 pages) ── ACT ──> Phone
                   │ BACK: Home                            │ BACK: Rides               │ BACK: the ride
```

Home's last card, RIDES, names the last ride's distance (or how many rides are
saved, or NONE YET) and opens the list. Each card shows when the ride started
(OCT 17  7:12, or RIDE n when the clock was unknown), its recorded time and
distance where kept, and its average heart rate. MOVE focuses the next ride
(held, it pages four cards at a time), ACT or a tap opens it, BACK goes Home.
Without a saved ride the list says NO RIDES YET.

A saved ride opens on the summary's pages, read back from its journal:
SAVED RIDE and its start replace RIDE SAVED, and READING shows while it is
read. Laps read back from the ride (a ride recorded before laps were kept
reads --); climbs are not kept, so they read --. Ascent and the
elevation profile come from the altitude recorded with the ride, with the same
3 m band as a live ride; a ride saved before altitude was recorded estimates
its ascent from the recorded gradient and shows no profile. MOVE or a swipe turns the page, BACK returns to
the list, and ACT (an arrow cue) offers the ride to the phone. A ride whose
record cannot be read says RIDE UNREADABLE.

The list holds every saved ride of both journals (internal flash and the
memory card's RIDES volume), newest first. A ride index, kept in PSRAM and
rebuilt after each boot, reads each journal in the background a few records
per tick once the list was first opened, about 20 bytes per ride and up to 1,000 rides per journal (the
oldest drop out beyond that). Until it has read a journal, the last four rides
each journal's catalog keeps still show, and a newly saved ride appears at
once.

## Bests

The rider's all-time bests are 5 s, 1, 5 and 20 min power, the longest ride
by distance and by time, and the most ascent. Settings, PROFILE, BESTS lists
each with its value and the day it was set; BACK returns to Settings.

- Only a ride recorded live that finishes counts. A demo ride never does.
  Rides under 10 minutes can set a power best but not a distance, time or
  ascent one, so a test press is not the longest ride.
- A best is replaced only by a higher value; a tie keeps the older day.
- A power above what a person holds for that length (2,500 W for 5 s,
  1,500 W for 1 min, 800 W for 5 min, 600 W for 20 min) is a sensor glitch and
  never a best.
- They are ride history, not settings: a small file on the FILES volume
  beside the workout outcomes (`ride-recording.md`, "Personal bests"), saved
  when the ride is. VANA never formats the store: where there is no card, no
  FILES volume or no formatted store (formatting is the host's, behind
  `--confirm`, and erases the bests), they are not kept: BESTS says BESTS
  NOT KEPT, and a ride is never held up for it. A device that never recorded
  one shows NONE YET.
- If the store would not take a write (full or failing), BESTS says BESTS NOT
  SAVED; the bests merged so far stay in memory and the next write keeps them.
  A read that failed only this time is tried again with the ride waiting.
- A file that cannot be read says BESTS FILE DAMAGED and is left alone.
- ACT on BESTS asks CLEAR BESTS?: ACT (check cue) clears them, replacing a
  damaged file too, and BACK (back cue) keeps them. A failed clear says CLEAR
  FAILED.

## Other routes

- **Phone.** `Page::Phone` is a placeholder for the phone companion: PHONE and
  SEND RIDES TO YOUR PHONE from Home, or SEND THIS RIDE TO PHONE from a saved
  ride; BACK returns where it came from. The phone workstream owns its Home
  entry and the sending.
