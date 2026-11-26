# VANA UX flows

This is the source of truth for how a rider moves through VANA: what the
rider sees and when, the order of screens and pages, timing, sounds and what
each button does. [UI](ui.md) owns how each screen looks (type, colour,
geometry and motion), and [ride recording](ride-recording.md) owns what is
stored. The simulator scenario
`the_ride_lifecycle_walks_from_home_to_ride_done` walks this flow and keeps
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
Home ── RIDE ──> Ride pages (recording)
                                                  │ ACT          ^
                                                  v              │ ACT (resume)
                                            Paused screen ───────┘
                                              │ swipe: ride pages, and back
                                              │ MOVE or BACK
                                              v
                                          END RIDE? ── MOVE or BACK ──> Paused screen
                                              │ ACT
                                              v
                                         SAVING ──> RIDE SAVED
                                                       │ any key or a tap
                                                       v
                                                      Home
```

Home's RIDE card starts the ride at once. Only the rider starts a ride:
rolling off on Home or Ride done starts nothing. There is no countdown: it
would delay the start at a light. Only storage stops a ride: when it cannot
record, RIDE shows STORAGE NOT READY and Home stays.

A live saved ANT+ sensor that reports a low or critical battery turns
Home's count and that sensor's dot amber, and Home's status line names it, such as HR LOW BATTERY. A sensor that
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
show cues: the buttons keep fixed roles. Holding MOVE for 1.5 s marks the
moment in the session trace ([Buttons](#buttons)) and leaves the page as it
was; `cycling trace pull` lists the marks.

`OPTION AUTO_LAP` takes OFF, a number of kilometres, or START, which
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

The CLIMB page has one layout on every road, which [UI](ui.md#charts) draws
under Elevation profile: the top line, then GRADE filled with its steepness
colour beside PWR 3S W, then HEART BPM beside CAD RPM, then two fields about
the climb, then a summary of three columns over the profile. What the climb
fields, the summary and the profile show depends on the road:

- **On the flat**: ALTITUDE M and ASCENT M. The summary and the profile are
  the last 2 km: LAST, AVG and MAX grade. All of it is behind the rider, so
  all of it is dimmed.
- **Climbing** without a map: the barometer's climb. LENGTH KM and HEIGHT M
  so far. The summary and the profile are the climb from its foot:
  CLIMB, its average and steepest grade, all dimmed.
- **On a course**: a climb from the map, ahead within 500 m or under way.
  TO TOP KM and TO TOP M. The summary describes the whole climb.
  The profile runs from the foot, or from the rider short of it, to the top.
  What is behind the rider is dimmed.

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
three quarters of the way down it ([UI](ui.md#map-page) gives the
geometry), so most of the map is the road ahead and,
while following, the blue route runs up through the middle. The course
ahead is a wide blue route on a dark casing, and a white arrow on the route
marks the next turn's corner. Only the course still to ride is drawn: from
the rider's place on it, forward along the course for 1.5 times the
distance the map shows ahead, so a road that winds a little still runs past
the top edge. A straight road needs 1.21 times that distance to pass the top
edge's far corner with its casing, and the mockup course, which jogs after
its turn, needs 1.33.
Off course the walk starts at the junction and goes that far past the
rider's distance to it. The course behind the rider is not drawn, and
neither is the ride's own track. The way back of an out-and-back shows only
when its turnaround is within that distance: then it is course still to
ride.

Before joining, or after following stops, the rider's place is the nearest
point on a leg of the course within 90° of the heading (the nearest point at
all when no such leg is within 25 m). The place keeps the leg it holds until
another leg is 10 m closer or the held leg is 25 m off on two moving fixes
in a row, or the heading is more than 135° against the leg on three moving
fixes in a row. So the route does not jump from fix to fix while the rider
stands still or wavers between two legs. Every fix counts, even one at the
same spot, whether or not the map is drawn again:

- A fix with a speed moves at 5 km/h or more (the speed the scale follows
  from) once it is 2 m on from the last fix that counted. At 5 km/h or more
  but less than 2 m on, it leaves every count as it is, so a slow rider
  still turns the leg.
- A fix without a speed moves when it is 2 m on from the fix before it, and
  travel never adds up across such fixes.
- A fix while standing (under 5 km/h, or without a speed less than 2 m on
  from the fix before) starts every count again. A moving fix that does not
  meet a rule starts that rule's count again, so a heading across the leg or
  a stop never changes the leg.

Each time the course is placed, replaced or removed, or a new ride starts,
the held leg is forgotten, even for a course of the same size.

The badge in the top-left corner shows the next turn and how far it is, at
rest too. It may
cover the route, never the puck: straight up from the puck it covers from
0.66 to 0.96 of the distance the map shows ahead, 184 to 267 m at 25 km/h,
so a turn there can be under the badge.

The map never pans or zooms to fit a turn or the route: its scale follows
the speed. From the puck to the top edge it shows about 40 s of riding,
never less than 150 m nor more than 1 km, so at 25 km/h it shows about
280 m. A ride starts at 300 m. Each fix moves the scale a fifth of the way
to that goal, so a change of speed eases in over several seconds and never
turns back; stopped, slower than about 5 km/h, or paused, it holds.

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
course follows from wherever the rider joins it. While following,
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
right after rejoining the course drops its rising beeps. How a navigation
sound waits behind other sounds is under [Sounds](#sounds).
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
A spur is where the course goes into a dead end and comes straight back
along it, as a waypoint just off the road draws it. VANA leaves a spur out
only where it is sure the dead end is a planner's artifact: the way back
retraces the way out to within 1 m, the dead end is 13 to 150 m deep, and the
way in, the spur and the way on leave its base at least 30 degrees apart.
There the course says what it would say with the spur's out-and-back taken
out: no turn into it, no U-TURN at its end and no turn out of it, each turn
after it as much further on as the spur is long. Wherever VANA is not sure,
the course keeps the spur as drawn: the turn in, the U-TURN and the turn
out. So at every spur the course says one or the other,
never a turn that neither says, and never leaves out a turn that both say.
VANA is not sure of a way back that drifts more than 1 m off the way out, a
dead end under 13 m or over 150 m deep, a fork narrower than 30 degrees, or
the far end of a longer out-and-back, where the way on runs back along the
way in. Nor is it sure of spurs within 250 m of one another, sure of each or
not, or of a spur within 250 m of an out-and-back's far end: it leaves every
one of them as drawn. The course keeps a spur's points and its length, so
distance along it and arrival are unchanged. Riding straight past the base
of a spur left out is not off course, and riding into it and back sounds
nothing either. Roads side by side about 8 m apart, a hairpin and a
roundabout keep their turns.
The turns found as the course loads are stored beside it. A course whose
turns file is missing, belongs to another course or cannot be read has its
turns and spurs found again from the course as kept, which for a long course
is thinned to fit.
Where the course is drawn coarsely, as a planner's route is, or its turns
are found from a thinned course, a turn of several corners is named only
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
in `docs/design/course-navigation` draw it, and main takes the mockups'
course layout ([UI](ui.md#course-sign) gives both): elapsed time and grade
on top, then a smaller speed, and power, heart rate and the lower line
further down. A band across the top of the page belongs to the sign, so the
sign comes and goes without moving or resizing a reading. A ride without a
course keeps the main page with the large speed.

- **At rest** (following, no turn near), main shows a chip under the time: the
  next turn's arrow on a blue square and how far it is (`6.4 KM`), or a flag
  and the distance to the end after the last turn. Climb shows nothing.
- **Turn ahead.** When the rising beeps sound, a blue banner covers the
  sign's band on main and climb: the turn's arrow and its distance in large
  digits (`120 M`). The time and grade under it are hidden, and on climb so
  are the readings it covers; nothing moves. The banner always shows
  the next turn: past a turn it shows the one after, and it stays up for 40 m
  past the turn before the next turn's own distance decides.
- **Off course**, the banner turns amber with a U-turn arrow, the straight-line
  distance back to where the rider left the course, and TURN BACK. Rejoining
  clears it.
- **Turn around.** Before following starts, riding against the course beside
  it turns the banner amber with the U-turn arrow and TURN over AROUND, on
  main and climb, and as the map's badge. It sounds nothing.
- **Arrived.** For 60 s after the end, the banner is green with a flag and
  ARRIVED, and main shows the ride's totals instead of live readings:
  DISTANCE, RIDE TIME, then AVG KM/H, AVG W and AVG BPM. Then the sign is
  gone.

Tapping the banner or the chip opens the map page. With a radar paired
the sign moves right with the page body, and the radar always keeps its
columns.

### Today's workout

VANA holds one workout, today's: `cycling workout-load` puts it there over
USB, or the phone page's Today's workout card does from the same JSON (or a
`.vw` file made by it). Each replaces the one before. Home's WORKOUT starts a
ride that plays it; with none loaded it shows NO WORKOUT LOADED and stays on
Home. The phone card shows the workout with its length and steps and a
Remove button, and says in words what it could not accept: not a workout,
damaged, one VANA cannot ride, no card or no room. A refused or cut-off
upload leaves the workout as it was. The page opens only while no ride is
recording, so the workout cannot change during a ride.

### Paused

A pause, by ACT or by auto-pause, fills the screen: two amber bars, PAUSED or
AUTO PAUSED, how long the pause has lasted, the ride time and the distance.
The pause chime plays once. An automatic pause adds RIDE ON TO RESUME, and
rolling on resumes it with AUTO RESUME. ACT resumes either pause (RESUMED
toast and the start chime). Riding on after a pause by ACT never resumes the ride, but VANA
plays a rising three-note reminder once the bike has held riding speed (6 km/h
on a wheel sensor, 10 km/h on GPS) for a few seconds, then at most once every 30 s
of that pause while the rider keeps riding, however often they stop and go. It
stops when the rider presses ACT. It sounds on any page, follows the SOUNDS setting
and plays with AUTO PAUSE off too, since every pause is then the rider's.

A swipe shows the ride pages from the page the rider was on, with PAUSED in
the status bar; paging past the last page comes back to the paused screen.
While paused, the keys never page.

### End ride

MOVE or BACK while paused asks END RIDE? with the ride time and distance.
ACT answers yes; MOVE or BACK answers no. The question never times out.
Answering yes finishes the ride, shows SAVING while the last samples are
written, then RIDE SAVED. Ending always saves, a ride of no distance too.

### Ride saved

Ending a ride saves it and goes Home, where the toast says RIDE SAVED. A ride
kept in memory that the phone has not saved says SYNC TO SAVE in amber
instead.

## Rides kept in memory

Some devices have no ride storage that survives power-off. Their internal
journal is in memory, and every ride recorded to it is lost when the device
turns off. Rides on a memory card are not affected. VANA protects each ride
in memory from its start until the phone saves a whole copy (see
[Connect phone](#connect-phone)), or until the rider clears it. While any
ride is protected:

- Home's status line says NOT SAVED, and the ride pages show a NOT SAVED
  toast. A low-storage toast takes its place while one shows.
- RIDE SAVED says SYNC TO SAVE in its place, and so does a toast that would
  say RIDE SAVED.
- The TURN OFF sheet says SYNC RIDE FIRST, or SYNC RIDES FIRST for more than
  one, in place of RIDES ARE SAVED, and shows no ACT corner. Every button
  closes the sheet, and the device stays on.
- The idle timer never turns the device off. It refuses with no message,
  because Home already says NOT SAVED.

A ride counts as saved once the phone page confirms it, and the ride then
shows as on any other device. The rider loses a protected ride only by
answering a CLEAR (below), or by a forced reboot: holding BACK for 10 s, a
crash or a power cut. A forced reboot loses every ride in memory, saved or
not.

The journal in memory takes 16 rides between clears, saved and unsaved
alike. After the 16th ride starts, RIDE starts no ride until the rides are
cleared.

A CLEAR of the journal in memory, from the phone page's **Clear rides** or
the terminal's `RIDE CLEAR CONFIRM`, empties it without a question when
every ride is saved. With unsaved rides the device first asks LOSE n
UNSAVED RIDES? (LOSE 1 UNSAVED RIDE? for one), over THEY CANNOT COME BACK
(IT CANNOT COME BACK).
ACT (red trash cue) clears the journal and the unsaved rides with it. BACK
keeps every ride, refuses the CLEAR and returns Home.

## Sensors

```text
Home ──> SENSORS ── ACT on ADD SENSOR ──────────────> NEARBY (searches)
   (straight to NEARBY              │ ACT on a sensor     ^          │ ACT on a sensor: connect it and
    when nothing is saved)          v                     │ BACK     │ stay; BACK: SENSORS, on it
                               Sensor's page ─────────────┘
                                 │ trash cue
                                 v
                               FORGET SENSOR?
                                 trash cue: forget; back cue: the sensor's page
```

SENSORS lists the saved sensors, each with its state, then ADD SENSOR. MOVE
walks the rows and wraps; ACT opens the focused one; BACK leaves for where
Sensors opened from. With nothing saved, Sensors opens straight on NEARBY,
and BACK there leaves too.

NEARBY searches as it opens, and only there: leaving it stops the
search. It lists each nearby sensor not saved yet, strongest signal first
(by signal bars, so rows do not swap on every packet). ACT on one saves it,
even when a sensor of its type is saved already: NEARBY stays open, counts
the sensors this visit added (2 ADDED) in green until the next message,
focuses the next sensor found and keeps listing the others, so several pair
from one search. BACK then returns to SENSORS with the sensor added last
focused. A sensor the search no longer reports says NOT SEEN - SEARCH
AGAIN. With all ten slots used, NEARBY says FULL - FORGET ONE. Once the
search is over, ACT with nothing listed, or a tap on the refresh mark,
searches again; with sensors listed, BACK and ADD SENSOR again does.

Several sensors of one type may be saved, such as two bikes' power meters.
The radio has one channel per type, so it connects the one used last. When
that one has sent nothing for a while (12 s, or 30 s while a ride records),
VANA runs its own 3 s discovery, at most every 30 s and never while the
sensor pages are open. It moves the channel to the saved sensor heard after
the active one fell silent, the strongest if several, and saves it as the one
used last. The radio connects nothing during those 3 s. A recording ride
never switches otherwise, and a discovery started before the ride stops when
it starts. The other sensors of the type say IN RANGE or NOT IN RANGE on
SENSORS.

A sensor's page shows its state, battery and number with button cues: back
beside BACK, and the trash beside ACT, or beside MOVE on a power meter, whose
ACT zeroes it (see The ride). The trash asks FORGET SENSOR? with the sensor's
name and number; ACT forgets it and returns to SENSORS, BACK returns to its
page. MOVE does nothing on these pages but a power meter's. A forgotten
sensor is never connected again until it is chosen on NEARBY.

## Connect phone

The terminal's `PHONE START` opens a phone session on VANA's own Wi-Fi
(`PHONE START HOME` on the home network), and `PHONE STOP` ends it. The
VANA iPhone app needs no session to be opened: `PHONE PAIR` pairs it, and a
paired device keeps a Bluetooth session open for it whenever no ride
records. The
phone's page lists the rides, downloads them as FIT files and loads a course
or a workout (see Course and Today's workout). The session also ends
after 10 minutes without a request from the phone, or when a ride starts. A
device without Wi-Fi has no phone feature.

On a device that keeps rides in memory
([Rides kept in memory](#rides-kept-in-memory)), the page marks each
unsaved ride with "Not saved yet: only this phone can keep it." Its
**Download .fit** button then saves the ride in four steps:

1. The page fetches the whole FIT file. VANA sends it with a receipt, a
   number drawn for this download only.
2. The page checks the file's FIT CRC against the CRC in its last two bytes.
   A short file, a wrong CRC or a missing receipt fails the save.
3. The page hands the file to the phone's browser to keep.
4. The page sends VANA the receipt and the CRC. VANA counts the ride as
   saved only when both match the newest download of that ride, so an older
   or cut download confirms nothing.

The page then says "Saved to this phone. VANA no longer needs to keep it."
Any failure says "Not confirmed:", the reason and "VANA keeps the ride. Try
again." A saved ride downloads as on any other device.

On such a device the page also shows **Clear rides** under the list, once
the list is read. It empties the journal in memory, so the next 16 rides can
start. With unsaved rides the page says "Confirm on the device." while
VANA asks LOSE n UNSAVED RIDES?, and "Rides kept." when the rider answers
BACK. A device whose rides survive power-off refuses a clear from the page,
and the page shows no such button.

## Buttons

Three buttons control everything, and each keeps one role on every screen:
BottomRight is ACT, BottomLeft is MOVE and TopLeft is BACK. Touch is a
shortcut for navigation only; [UI](ui.md#input) gives the gestures.

| Button | Home | Sensors | Ride |
| --- | --- | --- | --- |
| BottomRight (ACT) | Open the focused card | Open the focused row; on a sensor's page, zero a power meter or ask FORGET SENSOR?; answer a question | Pause, resume; on the NEW FTP sheet, save it |
| BottomLeft (MOVE) | Focus the next card; held 1.5 s: mark the moment (MARK n) | Focus the next row (wraps); on a power meter's page, ask FORGET SENSOR? | Next page (wraps); while paused, the paused screen is one of the pages (a manual pause, or an automatic one without a course, opens on it; an automatic pause on a course opens on the ride page); held 1.5 s: mark the moment (MARK n), except on the workout page during a running step, where it extends the step (+1 MIN, +0.5 KM), and on the map while following a course, where it stops following (COURSE OFF) |
| TopLeft (BACK / MARK) | Turn off, after the TURN OFF sheet confirms, on a device that can turn off | Back a page: SENSORS, then where Sensors opened from | LAP while recording, which during a workout also ends the step (next step); asks END RIDE? while paused; on the NEW FTP sheet, keep the old FTP |

| Button | End ride? | Ride saved |
| --- | --- | --- |
| BottomRight (ACT) | End and save it | Home |
| BottomLeft (MOVE) | Keep the ride, paused | Home |
| TopLeft (BACK) | Keep the ride, paused | Home |

The rider's mark writes the moment into the session trace, so whoever reads
the trace later starts where the rider noticed something. MOVE held for 1.5 s
marks once per hold, while the button is still down, with three middle notes
and a `MARK n` toast that counts the marks since power-on, so it confirms with
SOUNDS OFF too. The hold's first report is an ordinary MOVE; the mark puts
back the page or focus it changed, and the repeats before it change nothing.
The mark names the time of that first report and the page shown before it.
Lists keep their own held MOVE, so nothing marks there. Two
ride pages give the hold the job touch does there: on the workout page during
a running step it extends the step, and on the map while a course is
followed it stops following. Both put the page back as a mark does, and mark
nothing.

Holding TopLeft for 10 s reboots the device from any state, even one that
draws nothing. The device firmware watches for it below VANA, so a stuck
screen cannot swallow it. The hold's first report still acts as a press
(LAP, or the TURN OFF sheet on Home); a ride in progress resumes as a
recovered ride.

## Sounds

Cues are short runs of three notes. VANA takes the buzzer's patterns of
150 ms or less as low, middle and high notes by pitch, and the longest
pattern of 300 ms or less as the alarm. A buzzer with one short pattern plays
it for every note. Rising runs mean go, falling ones stop.

| Event | Notes |
| --- | --- |
| Ride starts | middle, high |
| Resumes | high |
| Pauses, by hand or automatically | high, middle |
| Riding on while paused by ACT (see Paused) | middle, middle, high |
| Lap | middle |
| Ride saved | low, middle, high |
| Battery low or critical, and BATTERY n LEFT | low, low |
| Workout step's last three seconds | low, once a second |
| Workout step begins, workout ends or a test measures FTP (in place of the lap chime) | high, high |
| A limit broken (HR, power or cadence) | high, high |
| EAT AND DRINK | middle, middle |
| Following a course, or back on it | low, high |
| A course's turn 300 m ahead | low, middle |
| 30 m before the course's turn | high |
| Off the course being followed, and once again 15 s later | high, low |
| End of the course reached | middle, high (the ride-start chime) |
| Vehicle appears behind | the alarm |
| Nearest vehicle near (under 60 m or 7 s), repeating every 1.5 s | low, once |
| Nearest vehicle close (under 30 m or 4 s), repeating every 1 s | middle, once |
| Nearest vehicle passing (under 15 m or 2 s), repeating every 0.5 s | high, once |
| Road empty for 2 s after vehicles (ALL CLEAR setting) | low, high |
| The rider marks a moment (MOVE held 1.5 s) | middle, middle, middle |

Each note waits for the previous one's guard. A newer cue replaces one that
waits or sounds, and a cue that cannot start within 1 s of its event is
dropped. The radar's cues for vehicles are safety cues: while one waits or
sounds, every other cue is dropped, and only another radar cue replaces it.
SOUNDS OFF mutes every cue but those.

Navigation cues (off course, back on course, the turn ahead, the beep at a
turn and the end of the course) wait apart instead. One of each kind waits
until no other cue waits or sounds, however long that takes, and one cut off
by a newer cue waits again. Waiting cues play in this order: off course, back
on course, the beep at a turn, the turn ahead and the end of the course. Each
is dropped once it no longer holds: its turn is passed, the off-course or
back-on-course state it told of has ended, or the end of the course was
reached more than 10 s ago, since a later chime would sound like a ride
starting.

The nearest vehicle sets the radar's pace, by its range or its time to reach
the rider, whichever is nearer. A vehicle holding the rider's speed sounds
only the alarm. When the road empties, the next vehicle sounds the alarm
again.
