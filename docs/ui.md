# VANA UI

VANA has one visual language. A rider glances at a 240 × 320 screen for under a
second, often in sunlight, while moving. So every screen follows the same rules:
black space instead of boxes, a few large numbers, and colour that always means
something. This document defines those rules. The code that implements them
lives in `apps/vana/src/screens/chrome.rs` (geometry, palette, shapes, status
bar, toasts), `apps/vana/src/screens/canvas.rs` (a prepared screen of text
runs and shapes) and `packages/shell/src/rendering/font` (type).

Every screen uses it: Home, Sensors, Workouts, Ready, the ride pages, the
paused screen, End ride, the ride summary, Settings, the brand reveal, the power
sheets and the charge shown while the device is off on USB power. None draws
in the 5 × 7 bitmap font. The screens a ride passes through, in order, are
set out in [UX flows](ux-flows.md).

## Principles

1. **Black is the layout.** There are no boxes, grid lines or dividers. Space
   groups things. Cards exist only where something can be focused or tapped.
2. **One reading per glance.** Each page has one hero number. Everything else is
   at least one size step smaller.
3. **Colour carries meaning.** A value is drawn in its metric's colour, a state
   in its state colour, and anything else is white or grey. If a colour means
   nothing, use grey.
4. **Words are small capitals.** Labels and units are short upper-case captions
   placed beside or above the number they qualify, never inside a frame.
5. **Edges are soft.** Text, discs, pills and cards are anti-aliased against
   what lies behind them.

## Type scale

The typeface is Nunito (SIL Open Font License, `packages/shell/fonts/OFL.txt`).
Its rounded terminals are what makes the numerals read as VANA. Glyphs are
rasterized ahead of time into 4-bit coverage, with tabular digits so values do
not jitter as they change.

| Face | Nunito | Height | Use |
| --- | --- | --- | --- |
| `CAPTION` | 800, 13 px | 14 px, caps 11 | Units, labels, the status bar, chart annotations |
| `BODY` | 800, 18 px | 19 px, caps 14 | Card titles, toasts, sensor names, the clock |
| `TITLE` | 900, 34 px | 37 px, caps 26 | One headline per screen (RIDE, MY SENSORS) |
| `VALUE` | 700, 34 px | 29 px, digits 26 | Secondary readings: elapsed time, cadence, distance, grade |
| `LARGE` | 700, 50 px | 42 px, digits 37 | Primary readings beside others: power, heart rate |
| `HERO` | 700, 78 px | digits only | The one reading a page is about: speed on the main page |

`VALUE`, `LARGE` and `HERO` hold only ` +-./0123456789:%`. Units are a
`CAPTION` in `SECONDARY`, bottom-aligned with the number and 6 px to its
right. A missing reading shows `--` in the value's face and colour.

To change a face, edit the table in `packages/shell/fonts/generate.py` and run
`uv run packages/shell/fonts/generate.py`. The script rewrites `faces.rs`.
Every face costs flash: all six take about 50 KB together.

## Palette

The background is always `BLACK`.

| Name | Meaning |
| --- | --- |
| `WHITE` | Primary text and neutral values (distance, time, clock) |
| `SECONDARY` | Labels, units, secondary text |
| `MUTED` | Unfocused titles |
| `TERTIARY` | Unlit dots, unlit gauge segments, chart baselines, "no data" |
| `CARD` | A card's fill |
| `RAISED` | A focused card's fill, buttons, toasts |
| `ACCENT` | Focus rings and information |
| `GREEN` | Live, good, fixed, go |
| `AMBER` | Attention: paused, low, needs a check, an armed press |
| `RED` | Recording, danger, failure |

### Night

At night VANA keeps the same palette and lowers the backlight to at most 20%
(`app::night::NIGHT_PERCENT`). The background is already black, so the
backlight is what glares in the dark. A dimmer palette would make the panel
darker still but would also shrink the gap between `WHITE` readings and
`SECONDARY` labels, and between `AMBER` and `RED` alerts and everything else,
so it is not used: alerts keep their full contrast at night.

DAY / NIGHT in Settings → Display chooses AUTO, DAY or NIGHT. In AUTO the sun
decides: night begins once it is 4° below the horizon at the last fixed
position (mid civil twilight) and ends once it is back above 2° below, so a
reading near the line cannot flicker. The time is network time, or the GPS
receiver's date and time. Without a position, the local clock decides (night
from 20:00 to 7:00); with no clock at all, the last answer stays. The night
cap goes through the shell's backlight ceiling (`Shell::set_ceiling`), which
is never stored, so the rider's chosen brightness is unchanged.

### Metric colours

A metric's number always uses its own colour, on every page and in every chart.

| Metric | Colour |
| --- | --- |
| Speed | `SPEED` (sky blue) |
| Power | `POWER` (yellow) |
| Heart rate | `HEART` (pink-red) |
| Cadence | `CADENCE` (= `GREEN`) |
| Grade, elevation | `GRADE` (= `AMBER`) |
| Time, distance, laps | `WHITE` |

### Zone colours

`ZONES` is one ramp shared by power zones, heart-rate zones and climb
steepness: grey, blue, green, yellow, orange, red, purple (Z1 to Z7).
`zone_color(0)` (no profile or no reading) is `TERTIARY`. Heart rate uses Z1
to Z5 of the same ramp.

Climb steepness uses the ramp from blue up:

| Band | Colour |
| --- | --- |
| Descent | `ZONES[1]` (blue) |
| Easy, below 3% | `ZONES[2]` (green) |
| Moderate, 3–6% | `ZONES[3]` (yellow) |
| Steep, 6–9% | `ZONES[4]` (orange) |
| Hard, 9–12% | `ZONES[5]` (red) |
| Severe, 12% and more | `ZONES[6]` (purple) |

## Shapes

| Shape | Rule |
| --- | --- |
| Card | Radius 18, `CARD` fill. When focused, `RAISED` fill with a 3 px `ACCENT` ring (`chrome::card`). |
| Small card or button | Radius 12, otherwise the same as a card |
| Pill (toasts, chips) | Radius = half its height |
| Gauge segment | 5 px tall, 3 px gaps, radius 2 |
| Dot | 6 px page dots; an 8 px tone dot in toasts |
| Icons | Drawn in one colour on a card. A state badge (a green check) sits at the top-right. |
| Ring | An arc of a ring with round ends (`chrome::Ring`), running clockwise from 12 o'clock. VANA's one shape for progress, charge and time left: the brand mark, the turn-off countdown, the charge and the power symbol. A ring that shows a share stands on a 25% track of its own colour. |

Shapes are coverage functions (`disc`, `rounded`), blended over what lies
behind them, and painted per row by `paint_rounded` and `paint_disc`. Never
draw a 1 px outline as decoration.

### Button cues

A screen that asks for a decision points at the buttons that answer it with a
cue: a quarter disc of radius 42 in the screen corner beside the button, with
an icon and no words (`screens::cue`). The disc's colour says what the button
does, so the cue reads at a glance.

| Icon | Disc | Meaning |
| --- | --- | --- |
| Play | `GREEN`, black icon | Start or resume (ACT) |
| Check | `GREEN`, black icon | Yes, or Done (ACT) |
| Send | `GREEN`, black icon | Send a saved ride to the phone (ACT) |
| Stop | `RAISED`, white icon | End the ride: asks first (BACK) |
| Back | `RAISED`, white icon | No, go back (BACK) |

Only Ready, the paused screen, End ride, the summary and a saved ride show
cues. Ride pages
never do: their buttons keep fixed roles. Content stays clear of the cue
corners; each screen's layout test checks it.

## Cells

The stats, lap and climb pages list readings in cells: a `CAPTION` in
`SECONDARY` over its value in `VALUE`, in the metric's colour. Cells sit in two
columns from the body's 10 px margin, 48 px apart, so each caption has 7 clear
rows above it, and keep 8 px between the columns (`ride::cell`). A page that
cannot fit its readings at that rhythm shows fewer of them: space over
density. A value that does not fit its column steps down to
`BODY` on the same baseline. Captions are short enough for a column:
`DIST KM`, `AVG W`, `LAP BPM`, `KM/H DIFF`. A previous lap is named by its
number, `LAP 4`, as its toast named it.

A difference carries its sign: `GREEN` with "+" for a gain, `SECONDARY` with
"-" for a loss or no change. An easier lap is not a failure, so it is never
`RED`; red stays for warnings.

## Spacing and regions

| Region | Rows | Notes |
| --- | --- | --- |
| `STATUS_BAR` | 0–21 | Ride mark (red dot and REC, or amber bars and PAUSED), page dots, then GPS, battery and clock on the right. No divider. |
| `CONTENT` | 24–319 | Pages derive their boxes from it. They never hard-code the status bar height. |
| `TOAST` | 280–319 | The toast pill sits at 282–311. Pages keep their least important reading, or nothing, here. |

The screen margin is 10 px. Text inside a card is inset 16 px. Cards are 8 px
apart.

Nothing is placed at a fixed x without a width budget. Text that can grow
(readings, clocks, summaries) is laid out with `Line::fit`: measure it, take
the largest face from a short list that fits its box, and keep its baseline.
If the smallest face still does not fit, drop the least important neighbour,
such as a caption, before cutting text. Each restyled screen has a sweep test
with extreme values: 120.0 km/h, 1999 W, 220 bpm, 180 rpm, 1000 km, 99:59:59
and full sensor lists. The test asserts that no two glyph runs overlap, that
nothing leaves its region and that every reading shows whole. On ride pages, the radar strip takes the left 28 px only while a radar is
paired. Otherwise the body shifts 14 px to stay centred.

## Status bar and toasts

The status bar is the only chrome that is always on screen. It shows only state
the rider may need at any moment:

- whether a ride is recording, paused, or paused by auto-pause (amber bars
  and AUTO) while the rider looks at ride pages during a pause;
- which page is showing;
- GPS: grey while searching, green when fixed;
- battery: an icon filled to the charge, white, amber at 30% or less; below
  20% a red percentage replaces the icon. On USB power a green bolt sits
  left of the icon, which stays even when low, and its green fill climbs
  from the charge to full in four steps, one per frame, then starts again;
- the clock.

Items are laid out with measured widths, in priority order: the ride mark,
the clock, the battery, GPS, then page dots. Dots close up from an 11 px to an
8 px pitch before they are dropped, and an item that still does not fit is
left out. Items never overlap. While recording, every combination fits.

Messages appear as toasts: a `RAISED` pill with a tone dot and `BODY` text,
centred over the bottom of the content.

| Tone | Dot | Used for |
| --- | --- | --- |
| `Good` | Green | Recording, resumed |
| `Info` | Blue | Laps, recovery, neutral news |
| `Warn` | Amber | Low storage, rides falling back to internal flash |
| `Alert` | Red | Storage failures |

Only one toast shows at a time. From highest priority to lowest:

1. The lap just taken.
2. An automatic start or resume (AUTO START, AUTO RESUME), shown briefly.
3. Transient announcements, shown for 2 s: RECORDING, RESUMED, RIDE SAVED
   (when a ride ends away from the summary), RIDE RECOVERED. A pause has no
   toast: the paused screen says it. Battery news shows for 4 s, on any
   page: CHARGING 64% (`Good`) when USB power arrives, LOW BATTERY 20% (`Warn`) as the charge falls to
   20%, and CHARGE SOON 10% (`Alert`) at 10% and again at 5%. Each level is
   announced once per discharge; charging re-arms them. A device switched on
   already low says so only in its status bar. Ride alerts (`ride/alerts.rs`,
   chosen under Settings ALERTS and LIMITS) show here on ride pages, each with
   its cue: a limit broken for 10 s, HR HIGH 172, HR LOW 95, POWER HIGH 400W,
   POWER LOW 120W or CADENCE LOW 60, is `Warn` and repeats every 2 min while it
   lasts. EAT AND DRINK (`Info`) comes every chosen interval of ride time. The
   radar count names each new vehicle behind, 2 VEHICLES (`Warn`, no cue: the
   approach alarm already sounds), and ALL CLEAR (`Good`) follows 2 s after the
   last one passed. All are off by default except the radar count. Limits
   and reminders keep 3 s apart; radar alerts never wait.
4. On ride pages only, sticky storage faults: STORAGE ERROR, then STORAGE
   FULL, WRITE FAILED, STORAGE NOT READY. A storage write failure is never
   hidden: the only one item 5 may replace is the bulk failure that moved
   the ride to internal flash. Any later failure shows here.
5. Sticky low storage: under one hour left, the toast shows `REC 42M LEFT`.
   Otherwise, while bulk media cannot take rides and the ride page shows an
   internal ride, a `Warn` toast names it: READY INT, RECORDING INT, PAUSED
   INT, AUTO PAUSED INT, SAVED INT or RECOVERED INT. After a recovery, RIDE
   RECOVERED stays until recording resumes.
6. Auto-pause: AUTO PAUSED stays while the rider looks at ride pages during
   an automatic pause (the paused screen names it itself). A
   ride recording without wheel or GPS speed shows AUTO PAUSE OFF for its first 8 s
   of active time, not for the whole ride.

There are no bottom button labels. Each button has a fixed role, so the screen
does not need to explain them; where a screen asks a question, a button cue
points at the answer.

### Ride chimes

Short chimes mark the ride's moments, so the rider need not look. Each uses a
platform pattern no longer than 150 ms; the 300 ms pattern stays the radar
alert's, and an alert always wins over a chime.

| Moment | Chime |
| --- | --- |
| Recording starts or resumes | The highest pattern |
| Paused (manual or automatic) | The lowest pattern |
| Lap | The shortest pattern |
| Ride saved | The highest pattern |

## Charts

Charts follow the same rules as the rest of the screen. VANA charts have no
axes, ticks, gridlines, legends or frames.

- **Baseline.** At most one 1 px `TERTIARY` baseline, and only when the zero
  matters (an elevation profile). Bars stand on black.
- **Colour encodes the category, height or length the quantity.** Use
  `ZONES` for zones and steepness, and the metric colour for a single metric.
  Never use a gradient that means nothing.
- **Past and future.** What has happened is drawn in full colour. What is
  ahead, or is only a reference, is drawn in that colour at 25% (`blend(color,
  BLACK, 4)`), or in `TERTIARY` when no colour applies.
- **Now.** The current position is a 6 px `WHITE` disc, or a `WHITE` bar,
  never a line across the chart.
- **Labels.** `CAPTION` text only at the ends (start, top, now, best), in
  `SECONDARY`. The one number the chart is about goes above it in `VALUE`
  in its metric colour.
- **Geometry.** Bars are 5 px (inline) or 8 px (standalone) thick with radius 2
  and 2–3 px black gaps. Areas are filled solid with no outline. Charts span
  the content width minus the 10 px margins.

Applied to the cycling-specific charts:

| Chart | Design |
| --- | --- |
| Zone gauge (main page) | One segment per zone, 5 px. Zones below the current one are lit in their colours. The current zone fills left to right as the reading moves through it. Zones above are `TERTIARY`. The zone chip (`Z4`) beside the reading is `BODY` in the zone colour, `Z-` without a zone. |
| Time in zone (stats) | One 8 px bar per metric, `PWR` and `HR` captions on its left, segments proportional to time in each zone, in zone colours, with 2 px gaps and no empty zones. A `TERTIARY` track before any time is recorded. |
| Elevation profile (climb) | A solid area per column, coloured by the steepness of that column's grade, on one `TERTIARY` baseline. A 5 px bar under the hero shows the current grade's steepness colour. **Without a map** it covers the last 4 km in full colour, with a 6 px `WHITE` disc on the newest column; the cells above carry the gain and length, and a `SECONDARY` "NO MAP" caption marks a missing terrain pack. **With a map** (`docs/terrain.md`) two rows of cells leave the profile a taller third row. "CLIMB AHEAD" runs from the rider to past the top, with the road before the foot at 25%. "TO THE TOP" keeps what was climbed at 25% behind the rider's disc, with the road ahead in full colour, and says "AND MORE" when the map's horizon ends before the top. |
| Workout (workout page) | One bar per played step across the content width, with a 1 px gap. Its width is proportional to the step's planned time (distance and open steps get an estimate), its height to the target's intensity, and a ramp rises across its width. The colour is the zone of the target's middle, `CADENCE` for cadence and `TERTIARY` without a target. Steps done and on now are full colour, steps ahead 25%. A `WHITE` bar under the chart marks the step on now. Above the chart, the step's band gauge: a `TERTIARY` track, the target band in its metric colour, and a `WHITE` disc at the 3 s reading. |
| Target band (main page) | During a workout, a 2 px `WHITE` bar under the power or heart gauge spans the step's target. |
| Lap comparison (laps) | One vertical bar per lap across the page's full width, each centred in an equal slot, up to 24 px wide with at least 3 px between bars, on one `TERTIARY` baseline. Height is proportional to the lap's average power, or its speed on a ride without power, in that metric's colour. Finished laps are drawn at 25%, the open lap in full colour, and the best lap gets a 2 px `WHITE` cap. The oldest lap is on the left and the open lap on the right; the 16 kept laps and the open one fit. |

The time-in-zone bars, the profile and the lap bars reach into the toast band.
They are the least important part of their page, and a toast covers them for
a moment only.

## How data updates

- **Prepare once, paint per row.** On every frame, `prepare()` turns the view
  into positioned labels, bars and shapes. `paint(row)` then fills one display
  row at a time: items whose rows it misses cost one comparison, and the rest
  paint spans. `Label::paint` blends a glyph run over what the row already
  holds. `paint_rounded` and `paint_disc` fill solid parts as spans and work
  out only anti-aliased edges per pixel. Paint backgrounds first; later
  paints cover earlier ones. Each painted screen keeps its per-pixel original
  as a test reference, and tests assert whole frames match it.
- **Values change in place.** Digits are tabular and right- or left-aligned to
  fixed anchors, so a number never shifts its neighbours. A reading moves
  only by gliding (see Motion).
- **Missing data looks dim, not alarming.** A stale or absent reading shows `--`.
  Zone colours fall back to `TERTIARY`. Errors belong in toasts or the status
  bar, not in the reading's place.
- **State changes announce themselves once.** Starting, pausing, resuming,
  laps and saving each raise a toast. Persistent conditions use the sticky
  toasts above or the status bar.

## Motion

Motion marks the device's own moments. Each is a ring that moves and text
that fades, prepared like any other frame, so a motion frame costs what a
still one does (about 10 ms on the C606). Motion frames draw every 40 ms; the
charging icon and sheets step every 500 ms; still pages redraw once a second.

Readings glide; nothing else about them moves. Sensors report about four
times a second. On the ride screen, speed, power, heart rate and cadence, and
the zone gauges that follow them, ease from the drawn value to each new
reading over 200 ms. They move fast at first, never pass the reading and land
on it exactly. Between reports, radar targets keep closing at their reported
speed for at most 300 ms, then glide to the next report. The ride screen
draws every 40 ms while something glides. Only drawing glides: recording,
totals and alerts use readings as they arrive. The mechanism is
`firmware_shell::rendering::glide`; VANA's use of it is in `app/motion.rs`.

| Moment | Motion | Duration |
| --- | --- | --- |
| Power on | VANA shows on the first lit frame; an `ACCENT` ring closes clockwise round it from 12 o'clock and LABS fades in. Home opens when it ends. | 400 ms, the existing brand time: Home is not later |
| Turn off | The ring's tail chases its head round until it is gone, while the wordmark and TURNING OFF fade to black. VANA asks the device to turn off once the black frame is on the panel. | 480 ms |
| Idle warning | An `AMBER` ring round the seconds empties with them. | 30 s |
| Charging while off | A `GREEN` ring shows the charge with its percentage inside, and a 50% green arc climbs from the charge to full as the status bar's icon does. CHARGED at 100%. The screen goes dark after 20 s; holding top left starts VANA as always. | 20 s |
| Day to night | The backlight steps 4% every 100 ms from the rider's brightness down to the night level, and back up in the morning. Nothing is redrawn. | About 1 s at 50% |

## Settings

Settings is a list of sections, one per page, with page dots in the status
bar: PROFILE, DISPLAY, RIDE, SOUND and DEVICE. A section has a `TITLE`
headline and at most four rows. A row is a small card (radius 12) with its
label as a `CAPTION` over its value in `BODY`, and an optional note in
`SECONDARY` at the right of the value line. A note that does not fit whole is
left out. The focused row is `RAISED` with a 3 px `ACCENT` ring.

Acting on a row does one of three things:

- a number (FTP, LTHR) opens the editor: one `ACCENT` field with the value in
  `VALUE` and the zone it anchors, over minus and plus pads;
- a choice steps to its next value at once (brightness 25/50/75/100%, dim
  after 15 s/30 s/1 min/5 min/never, day/night auto/day/night, auto-pause on/off, auto lap off/1/2/5/10
  km, sounds on/off);
- an action opens a page (SENSORS) or a flow (PHONE, a placeholder until the
  phone companion lands).

Storage and firmware rows only show facts. The save status sits in the
bottom band, under any toast. Brightness and dimming belong to the shell, so
VANA asks it to apply them at once and store them; the other choices are
stored in VANA's preference record.

## Sounds

Cues are short runs of the buzzer's three short patterns, taken as low,
middle and high notes by pitch. Rising runs mean go, falling ones stop.

| Event | Notes |
| --- | --- |
| Ride starts | middle, high |
| Resumes | high |
| Pauses, by hand or automatically | high, middle |
| Lap | middle |
| Ride saved | low, middle, high |
| Battery low or critical | low, low |
| Workout step's last three seconds | low, once a second |
| Workout step begins, workout ends or a test measures FTP (in place of the lap chime) | high, high |
| Vehicle closing from behind | the long alarm pattern |

Each note waits for the previous one's guard. A newer cue replaces one in
progress, and a cue that cannot start within a second is dropped. SOUNDS OFF
mutes cues; the radar alarm always sounds and nothing replaces it.

## Input

Three buttons control everything. Touch is a shortcut for navigation only.

| Button | Home | Sensors | Settings | Workouts | Ride |
| --- | --- | --- | --- | --- | --- |
| BottomRight (ACT) | Open the focused card | Act on the focused tile | Act on the row (edit, step a choice, open); while editing, raise it | Choose the focused workout, then Ready | Pause, resume; on the NEW FTP sheet, save it |
| BottomLeft (MOVE) | Focus the next card | Focus the next tile | Next row, into the next section; while editing, lower it | Focus the next workout (wraps) | Next page (wraps); while paused, the paused screen is one of the pages |
| TopLeft (BACK / MARK) | Turn off, after the TURN OFF sheet confirms | Back to the tiles, then Home | Home; while editing, finish | Home | LAP while recording, which during a workout also ends the step (next step); asks END RIDE? while paused; on the NEW FTP sheet, keep the old FTP |

| Button | Ready | End ride? | Summary | Rides | Saved ride |
| --- | --- | --- | --- | --- | --- |
| BottomRight (ACT) | Start the ride | End and save it | Done: Home | Open the focused ride | Send to the phone |
| BottomLeft (MOVE) | — | — | Next page (wraps) | Focus the next ride (wraps) | Next page (wraps) |
| TopLeft (BACK) | Home, dropping a chosen workout | Keep the ride, paused | Home | Home | Back to Rides |

| Gesture | Rule | Effect |
| --- | --- | --- |
| Tap | Never moves 16 px from where it started, at any point; lifted within 600 ms | Opens the card or tile under the finger on Home and Sensors, or the ride under it on Rides; on Ready, the sensors line opens Sensors and the workout line under the headline opens Workouts to change it; on Workouts, the workout under the finger is chosen. Tiles own their bounds; the search button's wider touch margin takes only spare space. On the workout page, a tap on the time left or its bar extends the step by a minute (500 m for a distance step) |
| Swipe | Moves at least 48 px horizontally and at least twice as far as vertically, within 1 s | Next page (right to left) or previous page (left to right) on ride pages, the summary and a saved ride; next or previous section on Settings |
| Settings rows | Tap | Act on the row, as ACT does |
| Settings pads | Act on contact | Step the edited value |

Touch never starts, pauses, laps or ends a ride. Wet gloves, rain and bumps
cause stray contacts, and a ride must not depend on them. Anything touch can do,
a button can do too.

A contact belongs to the screen it began on. Every touch edge reaches the
recognizer, even while a sheet owns the buttons. If the page, its subview (ride
page, sensor detail or replacement page, Settings edit mode) or a sheet changes
before the finger lifts, that contact gives nothing. A `Cancel` during a contact
drops the rest of it, however long the finger keeps sliding. A `Cancel` with no
finger down changes nothing for touch, so the next tap still counts. On Sensors
it still reaches the menu, which then waits for MOVE before ACT can change a
sensor.

Producers and the recognizer share one contract, set out on
`device_api::input::Input::Cancel`. After a `Cancel` the producer sends nothing
more from that contact. It sends one `Release` when the panel reports the lift
or stops reporting the finger for `STALE_CONTACT_MS` (250 ms). A finger the
panel stops reporting is cancelled and released at once, so a tap right after
it is whole. The C606 driver keeps this contract with
`firmware_services::input::TouchContact`.

Every edge queue keeps the contract too. When a slow consumer overflows
`Edges`, the queue drops its history and sends a `Cancel`. It then drops the
points of the contact in progress until that contact's `Release`, and tells the
producer, which cancels its own contact. A retained move can never reach the
recognizer as a fresh touch.

A producer that cannot keep the contract still loses only the cancelled
contact: a `Touch` that arrives at least 250 ms after the last touch edge
starts a new contact.

The radar track ends above the toast band. A toast never hides a radar target.

## Testing screens

The simulator reads the screen back as text, so scenarios assert what a rider
sees: `expect_text`, `field("W")`, `elapsed()`, `page()`. Anti-aliased text is
matched against glyph templates rendered from the same faces. The reader
tolerates glyphs that touch, as in `TT`. If a new layout makes two labels
ambiguous, rename one rather than weakening the reader.

`mise run sim` writes screenshots and videos to `.local/sim/latest`.
`mise run sim-compare --pr <n>` renders the same scenarios on main and on a PR,
side by side.
