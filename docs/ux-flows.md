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
                                              │ ACT      │ MOVE
                                              │          v
                                              │      DISCARD RIDE? ── ACT ──> Home (RIDE DISCARDED)
                                              │          │ BACK: END RIDE?
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
| Sensors | n SENSORS LIVE, with their short names | SENSORS WAKING UP, naming the missing ones; NO SENSORS, tap here to pair one; HR LOW BATTERY or n BATTERIES TO CHANGE, once every sensor is live | HR CRITICAL BATTERY, change it before riding | |
| Battery | BATTERY n%, with ABOUT n H LEFT once the drain rate is known | under 30%: charge after the ride | under 15%: charge before riding | |
| Storage | ROOM FOR n H | ROOM FOR n MIN or UNDER 1 MIN OF ROOM: export old rides; on internal memory | STORAGE FULL, STORAGE PROBLEM | |

The headline sums them up: READY when every line is green or grey, ALMOST
READY when something is amber, and NOT READY (red) when storage cannot record.
Only storage stops a ride; everything else is advice, so a rider is never
held at a green light by a sensor. Under the headline, the ride type reads
FREE RIDE, or the chosen workout's name.

ACT starts the ride (the green play cue points at it); there is no
countdown, because a countdown delays the start at a light, and rolling off
starts the ride from Ready anyway (auto-start). BACK returns Home. Tapping
the sensors line opens Sensors. A ride that started a
moment before the press (rolling off) is only shown, never paused, by ACT.

A live saved ANT+ sensor that reports a low or critical battery turns the
sensors line amber (red when critical), Home's count and that sensor's dot
amber, and Home's status line names it, such as HR LOW BATTERY. A sensor that
is not live is named first: a dead strap is missing, not low. Its row on
SENSORS says BATTERY LOW, and its page shows the voltage, such as 2.4 V, in
amber (red when critical). A
sensor with several batteries counts as its weakest. Levels are read from the
sensor's battery page as it sends it, kept only while VANA runs, and never
stored.

The selected BLE heart-rate or cadence sensor joins the same line, count and
status once it is selected (live while connected, left out when the radio
failed). A live saved ANT+ heart-rate sensor stands for the role and hides it,
as the ride prefers that sensor. The ride reads cadence from the BLE sensor
first, then a saved ANT+ cadence or speed and cadence sensor, then the power
meter's crank. A live ANT+ cadence sensor hides the BLE cadence sensor only
while that one is not connected and the crank has turned, so a sensor whose
cadence magnet is missing hides nothing. A BLE sensor's battery is the standard Battery
Level, read when it connects and every minute after: under 20 % is low and under 5 % critical, a warning
needs 2 % more to clear, and a value over 100 or a sensor without the Battery
Service says nothing. There is no detail page for it.

A saved power meter's page puts a green target cue beside ACT and the trash
cue beside MOVE. ACT does nothing for a second after the page opens, so two
quick ACT presses never zero a meter under a foot. ACT then sends the power
meter a manual zero request. WAITING FOR RADIO shows
while a search still holds the radio (up to twenty seconds), then CALIBRATING...
while the power meter answers (up to ten). The result is CALIBRATED with the
offset the power meter reported (CALIBRATED OFFSET 512, in its own units), or CALIBRATION
FAILED with a grey line, CRANKS STILL, TRY AGAIN, when it refuses, stays silent
or cannot be reached. NOT CONNECTED says the power meter is not live, NOT DURING
A RIDE says a recording is open (a request still waiting for the radio when a
ride starts is dropped, and so is one left behind by leaving the page), and
RESTART TO CALIBRATE says the radio keeps an earlier message of unknown fate
and refuses repeats until it restarts. The result stays for ten seconds and is
never stored.

Power meters that measure at the crank with torque frequency (SRM style) answer
a zero request with calibration ID 0x10 and their own offset page. VANA does not
decode that answer, so such a meter shows CALIBRATION FAILED after ten seconds
even when it zeroed.

### Recording

Starting switches straight to the ride pages, shows a RECORDING toast and
plays the start chime. On the ride pages ACT pauses, MOVE turns the page and
BACK takes a lap (with a lap toast and the lap chime). The ride pages never
show cues: the buttons keep fixed roles.

Settings RIDE has AUTO LAP: OFF, EVERY 1, 2, 5 or 10 KM, or AT START, which
takes a lap each time the rider rides past where the ride began, for loops
and repeats. It shows the same lap toast. The start is where the rider first
moves, so a car park off the road does not matter. A pass is within 40 m of
the start and then out beyond 50 m while moving, and counts once the rider
has been 100 m away; the lap closes as the rider leaves, so waiting at the
start, at a light or to refill a bottle, delays it without losing it, and
stopping there for good takes no lap. The first lap closes no earlier than
500 m and a minute; later laps have no minimum, so a short loop laps every
time. A paused ride takes no lap. A pass made wholly inside a GPS gap is not
counted. The ride started next after a restart that cut one short counts
from where the cut ride began, as the track below explains. GPS that drifts by
tens of metres between visits can miss laps; the lap count is a
convenience, and BACK takes one by hand.

### Climb

The CLIMB page has one layout on every road. The grade fills the top left
in its steepness colour, with no caption. POWER 3S and HEART stack beside
it, each with its zone on a chip in the zone's colour. A band in the grade's
colour runs under them, and two readings and a profile with a white disc
for the rider follow:

- **CLIMB IN 0.3 KM**: a climb from the map starts within 500 m. LENGTH KM,
  with AVG grade and GAIN M stacked beside it. The profile runs from the rider
  past the top, with the road before the foot and after the top dimmed.
- **CLIMBING** on a climb the map knows: TO TOP KM and UP TO GO M. The profile
  is the whole climb from its foot past its top, and it stays put while the
  rider's disc moves along it. What is behind is dimmed. A bar under it, FOOT
  to TOP, fills with the share climbed, `60% UP`.
- **CLIMBING** without a map: the barometer's climb. SO FAR KM and GAIN M over
  the last 4 km, with the road before the climb dimmed. The top is unknown, so
  there is no bar.
- **NO CLIMB**: ALTITUDE M and ASCENT M over the whole ride's elevation, with
  the rider at its end.

The category (CAT 4 to HC) sits right of the title in the climb's steepness
colour, and NO MAP sits there when there is no terrain pack.

The page comes up by itself when a climb is under way or within 500 m, as
long as CLIMB is on. Once the road is clear the ride screen goes back to the
page the rider was on. A rider who pages away during a climb stays where they
went until the climb is over. A rider already on the page stays on it after
the climb. Going straight over a top into the next climb counts as a new
climb. Paused or off the ride screen, nothing switches. Once the ride
screen opens again, a climb under way brings the page up.

### Map

The last ride page is the map. It fills the screen between the status bar
and a black footer, heading up: the way the rider is heading is always up,
and when the rider stands still the last heading holds.

The rider's puck sits in one place on every fix: centred across the map and
three quarters of the way down it, so most of the map is the road ahead and,
while following, the blue route runs up through the middle. The course
ahead is a wide blue route on a dark casing, and a white arrow on the route
marks the next turn's corner. Only the course still to ride is drawn: from
the rider's place on it, forward along the course for 1.5 times the
distance the map shows ahead, so a road that winds a little still runs past
the top edge.
Off course the walk starts at the junction and goes that far past the
rider's distance to it. The course behind the rider is not drawn, and
neither is the ride's own track. The way back of an out-and-back shows only
when its turnaround is within that distance: then it is course still to
ride. Before joining, the rider's place is the nearest point on a stretch
of course that runs the rider's way; it holds while the rider stands still
or wavers between two stretches, so the route does not jump from fix to
fix.
The badge in the top-left corner shows the next turn and how far it is, at
rest too; with a radar paired it sits 8 px right of the radar strip. It may
cover the route, never the puck: straight up from the puck it covers from
0.66 to 0.96 of the distance the map shows ahead, 184 to 267 m at 25 km/h,
so a turn there can be under the badge.

The map never pans or zooms to fit a turn or the route: its scale follows
the speed. From the puck to the top edge it shows about 40 s of riding,
never less than 150 m nor more than 1 km, so at 25 km/h it shows about
280 m. A ride starts at 300 m. A change of speed eases the scale in over several seconds,
never back and forth; stopped, slower than about 5 km/h, or paused, it holds.

Off course, the badge turns amber, the puck's arrow turns amber, and up to
three amber chevrons lead back in a straight line towards the junction where
the course left off, at the same scale and with the puck in the same place.
When the junction is off the map the chevrons point to where that line
leaves it, clear of the puck, the badge and a radar strip whichever way it
lies; the ringed junction shows once it is on the map, and the route goes on
from there, and only forward. Arrived, the badge is green.

Without a course loaded, the map draws the ride's own track behind the
rider in a thinner, dimmer blue. A loaded course not being followed, before
joining it or after stopping, is drawn in the route's blue from its point
nearest the rider, only the way the course runs.

The footer holds the scale bar (50 M, 100 M or 250 M) and, while following,
the distance left on the course; otherwise the ride's distance. Without a fix
the page says NO GPS YET and MAP STARTS WITH GPS; once a fix is lost the map
stays where it was and the puck's arrow turns grey. A device with no memory
for the track says NO TRACK.

The track is the ride's GPS breadcrumb, thinned to its bends, up to 4,096
points in PSRAM. Each ride starts a new one. After a restart cuts a ride
short, the ride started next reads the cut ride's track and start back from
its journal, a few records a moment, and carries them on if its own first fix
comes within 30 minutes of the cut ride's last. Until that is read, the map
draws the track read so far; a pass of the start then takes no lap. The
course is followed again only as on any ride: by riding along it.

A long press anywhere on the map page (a second), the badge included, stops
following the course; a tap does nothing, so rain or a glove cannot.

### Course

The phone page loads a course: a GPX track or route, up to 4 MB, which VANA
keeps on its memory card until the next one replaces it or the page clears
it. Riding along the course in its direction, within 25 m of it for a few
fixes, follows it: FOLLOWING COURSE and the rising cue.
Crossing it, or riding it the wrong way, follows nothing, and a course far
from the ride says nothing. Riding the wrong way within 25 m of it for three
moving fixes says TURN AROUND (below), silently, until the rider rides its
way, leaves it or loses the fix. A moving fix is at least 2 m on from the
last one, at 5 km/h or more when the ride's speed is known, so a fix
jittering at a stop never counts. Until following starts, the map draws small
arrows on the course's line near the rider, the way it runs, so a rider
standing at a junction sees which way to go. Riding another part of the same
course follows from wherever the rider joins it, as before. While following,
the rider is off course once they are more than 40 m from it on two fixes at
least 5 s apart, with no fix nearer between: two falling tones, and once more
15 s later. A fix lost for 5 s starts that count again. While the ride is
paused, by hand or by itself, nothing confirms or repeats off course, since
the bike may be wheeled anywhere; after the ride resumes the count starts
again from the next fix. However far off they ride, the course stays
followed and off course until they are back within 25 m of it, riding its
way: one rising pair of tones. Near its end, along it, following ends with
the ride-start chime. After that, or after the rider stops it on MAP, the
course is not followed again for the rest of the ride; the next ride follows
it again.

While following, each turn on the course sounds twice: two rising beeps when
it comes 300 m ahead, at any speed, and one beep 30 m before it. Turns whose
next one is within 300 m when the first's 40 m past it ends, or under 25 s
apart, are one run: the rising beeps sound for the first only, and each turn
of the run beeps 30 m before it. A turn first met nearer than 300 m sounds its
rising beeps at once, and one the rider is already in sounds nothing. At most
one navigation sound plays per fix: another due on the same fix waits for the
next, and is dropped if it no longer holds, so a turn beeping 30 m ahead
right after rejoining the course drops its rising beeps. A navigation sound
raised while a radar alarm or another sound plays waits and plays after it,
however long that takes, unless by then its turn is passed or the off-course
state it told of has ended. One of each kind waits at once; together they
play OFF COURSE first, then ON COURSE, the beep at a turn and the turn
ahead. A sound whose turn passes before the buzzer starts it never plays.
A turn is where the course turns by 60 degrees or more, turning at least as
sharply as 60 degrees in 50 m: LEFT or RIGHT, SHARP LEFT or SHARP RIGHT past
120 degrees, and U-TURN past 150. A bend gentler than that sounds nothing.
Each street corner is a turn of its own, sounded in order. Round a city
block, two corners the same way within a few tens of metres are one turn, the
way they go. A U-TURN is only where the way back is within about 20 m of the
way out, or the road curves round all the way, as a hairpin does. A
roundabout of up to about 40 m radius is one turn, named by the way it
leaves, and straight over one sounds nothing.
A course's turns are found once, as it loads from the phone, from every
point of the file, so a long course thinned to fit names them as drawn.
A spur is where the course goes into a dead end up to 150 m deep and comes
straight back along it, every point of the way back within about 3 m of the
way out, as a waypoint just off the road draws it. VANA takes every such
dead-end visit for a planner's artifact, even one meant: the course says
exactly what it would say with the spur's out-and-back taken out, so no turn
into it, no U-TURN at its end and no turn out of it. Every turn, its name,
and when its sign shows and sounds, is as on that course, each turn after
the spur as much further on as the spur is long. A corner at the spur's
base, or a jog just past it, is named as it would be without the spur. The
course keeps the spur's points and its length, so distance along it and
arrival are unchanged. Riding straight past the base is not off course, and
riding into the spur and back sounds nothing either. A spur is a detour off
a road the course rides on. A dead end whose way back runs on back along the
way in, within 20 m of leaving it, is the far end of a longer out-and-back,
even where the way back strays more than 3 m for a few metres. At the far end of a longer out-and-back, a dead end of up
to 30 m past its last corner is a spur, so the corner says U-TURN. A longer
dead end, roads side by side about 8 m apart, a hairpin and a roundabout keep
their turns. Known limit: where the road on from a spur's base hooks back
within 3 m of the way in for its first 9 m or so, and is drawn a point every
few metres, VANA takes those metres for more of the spur, and its turns there
may differ from the course without the spur. Turns stored with a
course loaded before this keep its spurs until the course is loaded again.
Where the course is drawn coarsely, as a planner's route is, or it was loaded
before turns were found this way, a turn of several corners is named only
roughly: LEFT or RIGHT from about 80 degrees, U-TURN from about 160, never
SHARP, and otherwise nothing; a roundabout there may say nothing but never a
wrong way. Wherever which way comes first is not clear, as when corners come
within a few metres of each other, or a turn's first bend goes against where
it leads, nothing sounds, and nothing after it either until the rider is
there. Off the course, riding it the wrong way, or once it is stopped or
ended, no turn sounds; riding back 30 m or more past a turn and on again
sounds it again. Where the course runs the same road twice, as a figure of
eight or an out-and-back does, the turns are those of the pass the rider is
on. A course with more than 1,024 turns sounds them to its end.

While a course is loaded, the data pages show one sign for it, as the mockups
in `design/course-navigation` draw it. Main takes their layout: elapsed time
and grade on top, then a smaller speed, and power, heart rate and the lower
line further down. Rows 22 to 104 belong to the sign, so it comes and goes
without moving or resizing a reading. A ride without a course keeps the page
it had, with the large speed.

- **At rest** (following, no turn near), main shows a chip under the time: the
  next turn's arrow on a blue square and how far it is (`6.4 KM`), or a flag
  and the distance to the end after the last turn. Stats and climb show
  nothing.
- **Turn ahead.** When the rising beeps sound, a blue banner covers rows 22
  to 104 on main, stats and climb: the turn's arrow and its distance in large
  digits (`120 M`). The time and grade under it are hidden, and on stats and
  climb so are the readings it covers; nothing moves. The banner always shows
  the next turn: past a turn it shows the one after, and it stays up for 40 m
  past the turn before the next turn's own distance decides.
- **Off course**, the banner turns amber with a U-turn arrow, the straight-line
  distance back to where the rider left the course, and TURN BACK. Rejoining
  clears it.
- **Turn around.** Before following starts, riding against the course beside
  it turns the banner amber with the U-turn arrow and TURN over AROUND, on
  main, stats and climb, and as the map's badge. It sounds nothing.
- **Arrived.** For 60 s after the end, the banner is green with a flag and
  ARRIVED, and main shows the ride's totals instead of live readings:
  DISTANCE, RIDE TIME, then AVG KM/H, AVG W and AVG BPM. Then the sign is
  gone.

Tapping the banner or the chip opens the map page. While a course is loaded,
the map page is in the MOVE cycle even if the rider turned it off, so the tap
always has a page to open and the page dots stay true. With a radar paired
the banner starts right of the radar strip and its contents move with the
page body; the radar always keeps its columns.

### Workouts from the phone

The phone page has a Workouts card beside the Course card: the workouts added
from the phone, each with its length and steps and a Delete button, and Add a
workout. The card takes the workout JSON `cycling workout-load` takes (or a
`.vw` file made by it) and shows what it could not accept in words: not a
workout, damaged, one VANA cannot ride, no card or no room. VANA holds 8
workouts, so a ninth is refused with "VANA holds 8 workouts. Delete one to add
another." rather than dropping one. Adding a workout with the name of one
already there replaces it, and a refused or cut-off upload leaves the stored
workouts as they were. The page opens only while no ride is recording, so a
workout cannot be added or deleted during a ride. A file in the workouts folder that VANA cannot read (or
that the card holds beyond the eighth) shows on the card as "Not listed on the
device" with a Delete button, since it takes one of the 8 places. A workout
loaded over USB and the same one added here are two workouts in the picker. An added workout is under
TRAIN at once, after the built-ins and in name order with the ones loaded over
USB; it reads the same as one of those, and a workout the rider chose stays
chosen when the list changes.

### Choosing the main page's readings

Settings has a MAIN PAGE section with LOWER LEFT and LOWER RIGHT, the two readings
on the main page's last line. They start as CADENCE and DISTANCE. A rider without
a cadence sensor can swap CADENCE for KCAL, NP, AVG HR, AVG SPEED, AVG POWER, WORK
or ASCENT, and the page shows `--` for one that has no data yet. The choice is read
when the ride screen opens, applies to every ride and survives a reboot.

### Choosing the ride pages

Settings has a RIDE PAGES section with an ON or OFF row for each page a rider
may not need: STATS, LAP, CLIMB and MAP. All are ON by default. MAIN
is always on, and so is the WORKOUT page while a workout is chosen. MOVE and
swipes step through the pages that are on, wrapping from the last to the
first, and the page dots count only those. While a course is loaded, MAP is
on whatever its row says (see Course). With every optional page off, MOVE
has only MAIN, and with a workout only WORKOUT and MAIN. When a workout
ends, a rider on the WORKOUT page moves to MAIN; the FTP ramp test ends with
its last step, so the NEW FTP sheet shows over MAIN.

CLIMB OFF also ends the climb page's habit of coming up by itself: neither the
ride's own climb detection nor a climb ahead on the map switches the page. A climb
never had a toast, and none is added: the rider declined the page, and a toast
would cover the readings they chose to look at. The CLIMB row's note
says NO CLIMB POP-UP while it is off. The choice is read when the ride screen
opens and applies to the next ride page the rider sees; it cannot change under
them, since Settings is not reachable during a ride.

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

A second amber toast with the same chime, TURN BACK BY 17:40 in local time,
says when to start home to be there by sunset. It speaks once per ride, on
the ride pages, when less than 15 minutes remain before that time; if the time
has already passed it says TURN BACK NOW. The time home is the straight
distance to the start, plus 30% for winding roads, at the ride's moving
average speed, but never more than the track ridden, which is always a way
back: exact on a straight out-and-back, and on a loop the track is no guide, as
it is far longer than the way home. Following a course does not silence it, since
its end need not be home; only heading for the start does. On very winding roads, or across a river or valley,
the straight line plus 30% underestimates, so the alert can come late. The
estimate waits for 5 minutes of moving and for 5 km/h or more.
The alert is part of ALERTS SUNSET, with no setting of its own. It says nothing
in polar day or night, after sunset, without a position or time, while paused,
at the start, or on a recovered ride until its track has been read back. It
also says nothing when riding towards the start (heading within a quarter
turn of it, on a course too), and holds while the
heading is unknown, as after a stop, until the rider moves again. It rarely
repeats: only after the margin has stayed above 30 minutes for 10 minutes.

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

With a card whose FILES store is formatted, a red trash cue sits beside MOVE.
MOVE asks DISCARD RIDE? with the same time and distance and IT WILL NOT BE
SAVED. ACT (red trash cue) discards the ride, BACK returns to END RIDE?. A
discard saves the ride as usual and then hides it (`ride-recording.md`,
"Discarded and deleted rides"), with no summary: Home shows RIDE DISCARDED,
and Home's card, the list and the week leave the ride out. It sets no bests
and keeps no facts. The same sheet follows an auto-pause (BACK took the pause
over, so rolling on does not resume the ride under it), a lap (the whole ride
goes, laps and all) and a 0 km test ride. If the ride cannot be hidden once
saved, Home says NOT DISCARDED and the ride is kept as any other, with its
bests and facts.

### Ride saved

The summary opens with a green check and RIDE SAVED, and pages through:

1. Time, distance, average speed, ascent, average power and average heart
   rate.
2. Normalized and maximum power, work in kJ, KCAL (the work again, as
   calories: 24 % efficient muscle makes kJ and kcal nearly equal; for the
   seconds without power, such as after a meter dies, the heart-rate estimate
   by the Keytel formula is added once Settings, BODY holds weight, age and
   sex, and nothing until then), average and maximum heart rate, power and
   heart-rate zone bars, and HR RECOVERY: the fall in heart rate over the
   minute after the ride stopped recording. A ride is paused before it ends,
   so it counts from that pause, with MEASURING and a countdown until the
   minute is up, then the fall (-28 BPM). A rider who waits more than five
   seconds past the minute before ending the ride, a sensor that is silent at
   either moment (ANT+ or BLE, ride open or not), or leaving the summary
   before the minute is up shows nothing. Once measured it is saved with the
   ride's facts (see Saved rides).
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
other three pages keep their buttons: ACT and BACK still go Home. Like END
RIDE?, it shows without the status bar, which would cut its back cue. On the
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
                   ^                                       │ MOVE past the last page
                   │                                       v
                   └──── ACT (RIDE DELETED) ──────────── DELETE RIDE?
```

Home's last card, RIDES, names this week's distance (WEEK 84 KM) once the ride
index has read both journals and the week has a distance. Otherwise it names
the last ride's distance, how many rides are saved, or NONE YET. This week's
distance is shown rather than the last ride's because it is the number that
survives a reboot and keeps meaning something between rides; the last ride's
distance is on the summary the rider just saw. The card opens the list. Each card shows when the ride started
(OCT 17  7:12, or RIDE n when the clock was unknown), its recorded time and
distance where kept, and its average heart rate. MOVE focuses the next ride
(held, it pages four cards at a time), ACT or a tap opens it, BACK goes Home.
Without a saved ride the list says NO RIDES YET.

While the focus is on the first three rides, a header sits above the cards:
THIS WEEK and the number of rides, the week's distance (KM) with its moving
time under it, then the week's TSS in the power colour (the training stress
of this week's rides that had power, on its own line under the time), a small
bar for each day (Monday to Sunday, every other day named; ridden days white,
today's ride blue, other days a dark stub) and, in grey, LAST WEEK with its
distance. From the fourth ride on the header gives
way to the RIDES headline and a fourth card. A week runs Monday 00:00 to the
next Monday in the status-bar clock's zone, and a ride counts in the week and
day it started. Rides without a start time (the clock was unknown) are in no
week. Totals come from the ride index across both journals, so rides read
back from the MMC and internal flash both count; a ride that moved from the
MMC onto internal flash is two journal rides, as the list shows it, adding
their times and distances without overlap and counting twice in the ride
count. Distance is the recorded speed added up (wheel sensor or GPS). While
the index is still reading, the header shows what it has found, or -- before
it has found anything; without a clock (network time, or the GPS receiver's
date from its last fix) it shows -- and no last week. Ride start times use the same clock, so a ride
dated by the GPS receiver counts too.

TSS adds each ride's training stress, worked out when the ride index reads
the ride (normalized power over 30 s, as the live ride's TSS) against the
FTP the ride was saved with, so changing FTP does not restate past weeks. A
ride without facts (below) counts at the FTP set now. A ride without a
power meter, or with under 30 s of power, adds nothing; a week with no ride that had power shows no TSS
rather than 0; more than 9999 reads 9999+. Rides the index has not read yet carry none, and nothing is
saved: the stress is rebuilt with the index after every boot. A 7-day against
42-day load line is not in this header, which has no room for it beside the
distance, the bars and LAST WEEK; it has its own page, FORM (below).

A saved ride opens on the summary's pages, read back from its journal:
SAVED RIDE and its start replace RIDE SAVED, and READING shows while it is
read. Laps read back from the ride (a ride recorded before laps were kept
reads --). Climbs and HR RECOVERY come from the ride's facts. Ascent and the
elevation profile come from the altitude recorded with the ride, with the same
3 m band as a live ride; a ride saved before altitude was recorded estimates
its ascent from the recorded gradient and shows no profile. MOVE or a swipe turns the page, BACK returns to
the list, and ACT (an arrow cue) offers the ride to the phone. A ride whose
record cannot be read says RIDE UNREADABLE.

With a card whose FILES store is formatted, a saved ride gets one more page
dot. MOVE past its last page shows, without the status bar, the ride's start,
DELETE RIDE?, its time and distance and IT GOES FROM YOUR RIDES. ACT (red
trash cue) deletes it and BACK returns to the list, as on every saved-ride
page. Like the FTP offer it is reached only going forward, so a swipe back
from the first page never lands on it. The list then says RIDE DELETED with
the focus on the ride that took its place, and the ride count, the week's
distance and TSS, FORM, Home's card and the phone's list drop the ride. The
journal keeps it (`ride-recording.md`, "Discarded and deleted rides"), and
the bests it set stay. Undated rides newer than it renumber (RIDE n counts
list places).

A ride's facts are what its journal does not hold: the FTP and LTHR in force
when it was saved, its HR RECOVERY once measured, and the climbs it finished.
They are a table on the FILES volume (`ride-recording.md`, "Ride facts"),
written when the ride is saved, that keeps the newest 1000 rides: FORM's 120
days at eight rides a day. A ride without them reads as before: TSS at the FTP
set now, CLIMBS --, and no HR RECOVERY. That is a ride saved with no card,
FILES volume or formatted store, one saved without a clock, one recovered after
a power cut, one older than the newest 1000, and one saved by firmware from
before facts were kept. A ride that moved from the MMC onto internal flash keeps its
facts with its internal part.

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
each with its value and the day it was set, and each power best also in W/KG
once Settings, BODY holds a weight; BACK returns to Settings.

- Only a ride recorded live that finishes counts. A demo ride never does,
  nor a discarded one. Deleting a saved ride keeps the bests it set.
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

## Form

Settings, PROFILE, FORM opens the rider's training load from the last four
months of rides with power. Its row reads the
current form number and word once the ride index has read the rides (it follows FTP
changes), and OPEN TO SEE until then. FITNESS is the 42-day
average of the daily training stress, FATIGUE the 7-day one, and FORM is
fitness minus fatigue, with a word for it: RESTED above +25 (white), FRESH
from +5 to +25 (green), EVEN from -10 to +4 (white), BUILDING from -30 to -11
(white, the normal cost of training) and VERY TIRED below -30 (red). Under them one bar
a week shows how form stood at the end of each of the last six weeks, up for
positive and down for negative, from 6 WEEKS AGO to NOW. BACK returns to
Settings.

- The stress is each ride's TSS (see the rides list), at the FTP the ride was
  saved with where its facts are still kept, else the FTP now, added up by the
  day the ride started in the status-bar clock's zone. A day with no ride
  counts zero, so both averages decay. A ride without power counts for
  nothing, and when there were any in the window the page says RIDES WITHOUT
  POWER ARE LEFT OUT.
- The averages need a starting value, and the first week of the 120 days
  gives it (its mean stress). Its effect fades with the 42-day average to
  0.057 of its error over the window: about 0.057 times the first week's mean
  minus the rider's usual load, so +11 if that week was a training camp. The
  numbers are an estimate until the rider has about four months of rides, and
  settle within about a week once an unusual first week leaves the window.
  Rides older than 120 days do not count.
- Opening FORM starts the ride index if nothing has yet, and the page says
  READING RIDES until both journals are read. Without a date it says NO DATE
  YET and GET A GPS FIX, and with no ride with power in the window, NO POWER
  RIDES and NEEDS A POWER METER.
- Nothing is saved: the page is rebuilt from the ride index and the facts
  after every boot.

## Sensors

```text
Home, Ready, Settings or setup ──> SENSORS ── ACT on ADD SENSOR ──> NEARBY (searches)
   (straight to NEARBY              │ ACT on a sensor     ^          │ ACT on a sensor: connect it and
    when nothing is saved)          v                     │ BACK     │ stay; BACK: SENSORS, on it
                               Sensor's page ─────────────┘          v
                                 │ trash cue                  REPLACE SENSOR? (a saved type)
                                 v                              check cue: replace; back cue: keep
                               FORGET SENSOR?
                                 trash cue: forget; back cue: the sensor's page
```

SENSORS lists the saved sensors, each with its state, then ADD SENSOR. MOVE
walks the rows and wraps; ACT opens the focused one; BACK leaves for where
Sensors opened from. With nothing saved, Sensors opens straight on NEARBY,
and BACK there leaves too.

NEARBY searches as it opens, and only there: leaving it stops the
search. It lists each nearby sensor not saved yet. ACT on one connects it:
NEARBY stays open, says HEART RATE ADDED (the sensor's name) in green until
the next message, and keeps listing the other sensors found, so several pair
from one search. BACK then returns to SENSORS with the sensor added last
focused. One of a type already saved asks REPLACE SENSOR first. A sensor the search no longer reports says NOT SEEN - SEARCH
AGAIN, and with all ten slots used, SENSOR SLOTS FULL. Once the search is
over, ACT with nothing listed, or a tap on the refresh mark, searches again;
with sensors listed, BACK and ADD SENSOR again does.

A sensor's page shows its state, battery and number with button cues: back
beside BACK, and the trash beside ACT, or beside MOVE on a power meter, whose
ACT zeroes it (see Ready). The trash asks FORGET SENSOR? with the sensor's
name and number; ACT forgets it and returns to SENSORS, BACK returns to its
page. MOVE does nothing on these pages but a power meter's. A forgotten
sensor is never connected again until it is chosen on NEARBY.

## Other routes

- **Phone.** `Page::Phone` is a placeholder for the phone companion: PHONE and
  SEND RIDES TO YOUR PHONE from Home, or SEND THIS RIDE TO PHONE from a saved
  ride; BACK returns where it came from. The phone workstream owns its Home
  entry and the sending.
