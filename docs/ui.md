# VANA UI

VANA has one visual language. A rider glances at a small screen for under a
second, often in sunlight, while moving. So every screen follows the same rules:
black space instead of boxes, a few large numbers, and colour that always means
something. This document defines how screens look: type, colour, shapes,
geometry and motion. [UX flows](ux-flows.md) owns what the rider sees and
when: the order of screens, timing, sounds and what each button does. The
code that implements these rules lives in `apps/vana/src/screens/chrome.rs`
(geometry, palette, shapes, status bar, toasts),
`apps/vana/src/screens/canvas.rs` (a prepared screen of text runs and shapes)
and `packages/shell/src/rendering/font` (type).

Every screen uses it: Home, Sensors, the ride pages, the
paused screen, End ride, the brand
reveal, the power sheets and the charge shown while the device is off on USB
power. None draws in the 5 × 7 bitmap font.

## The kit and its two looks

Kit screens are built from one small set of tiles, the kit
(`apps/vana/src/compose/kit`), drawn in one of two looks. The Stats
(`ride/main`), Climb (`ride/climb`) and Workout (`ride/workout`) ride
pages are three; Home and the swipe-down sheet draw in the looks too (see
below). Other screens follow the rules in the rest of this document.

- **Looks are data.** `screens/theme` holds one `Theme` of colours by role.
  `DARK` (night) is main's palette. `LIGHT` (day) writes every word and
  number black on white, and only bars, charts and zone or steepness fills
  carry colour. `LAYOUT` holds the gaps, radii and faces that both looks
  share, so a screen solves to the same boxes in both. The look in force
  follows `Runtime::light()`.
- **Tiles never name a value.** A kit tile reads its colours from
  `data.theme()` in `prepare`, by role or by `Tone`. It reads every gap,
  radius and face from `LAYOUT`.
- **Kit pages paint unmapped.** A screen that `ScreenId::themed` names
  fills the theme's background and paints its tiles without the light
  transform of `look.rs`. The radar strip, the course sign, toasts and cues
  still paint through the transform until they are rebuilt.
- **A tile's setting byte names a spec.** Each kit tile has a spec table:
  an enum with one fixed id per entry, whose `spec()` says what the entry
  draws. A tree reads as data, for example `field(Field::Pwr)`. Each screen
  family owns a block of ids in each table, and two entries with one id do
  not compile.
- **Tiles in the kit.** `field` is a ride-grid cell: a capital title that
  carries the unit, with an optional symbol and zone tag, over 40 px
  tabular digits (34 px where a chart shares the page), filled by zone or
  steepness with black ink, and a `%` at two-thirds size on the baseline.
  Beside a radar a field drops its symbol, then its wide margins, before
  its digits shrink. `road field` is a field that follows the road: a
  different field on the flat, on a climb and on a course's climb. `road
  profile` is a three-column summary over the climb chart. `top line` shows the recording dot and
  time, and the satellite and battery coloured by state. `rule` is a 1 px
  line in the rule colour. `text` is one line of words or a metric, in
  sentence case where the face has lowercase. `symbol` is one icon in a
  tone. `item` is an Apple-Watch-style rounded button with a focus ring and
  an action. `bar` shows zone segments, a level, or a workout step's
  target band: a wide track with the target range lit in its zone's colour
  over the middle half and the reading as a marker ringed in the
  background. `step` is a line of the workout step: the target range high
  over low with a rule in its zone colour between, the hero reading with
  its unit over `3s`, the time left, or the next step with its zone as a
  square. `chart` is the box a page's chart painter fills under the tiles.
  `facts` is an
  inset-grouped list. `kit::grid` lays out rows of cells with rules between
  them that line up. The kit's faces are under Type scale below.

### Home and the swipe-down sheet

Home is a watch face (`compose/builtin/home.rs`, tiles in
`compose/tiles/home.rs`, ids 107–110). The battery percentage sits top
right in its level's colour, then the date (`Tuesday 24 Nov`) and the time
in the 64 px clock face. Under them, one status widget: the satellite with
a dot for the fix, a rule, and the first three saved sensors in their metric
colours, each with a dot: green when live, amber while not yet or with a low
battery, red with a critical one (the sheet's Sensors row does the same). The start
is at the bottom. With a workout planned for today, its card comes first: a
zone 4 fill in black ink with a play disc, the name, `Today's workout ·
1:04` and a strip of its steps, each as wide as it is long, as tall as it is
hard and in its power zone's colour. "Just ride" sits under it. With nothing
planned, one big Ride button. Home opens on the card or on Ride every time,
with a ring 2 px outside it; MOVE steps to "Just ride" and then the widget,
and ACT or a tap runs what has the focus. The card starts the workout, and
Ride and "Just ride" start a ride with none, though one stays planned.
There is no Ready page and no fix to wait for. Rolling off on Home starts
nothing. BACK asks to turn off, and a refused turn-off shows as a toast.

The swipe-down sheet (`compose/tiles/sheet.rs`) paints its card in the
look's own colours over the dimmed screen. It shows the brightness bar with a
small sun and a big one, the Sensors row with each sensor's live dot and a
chevron, and the Day/Night tile, which names the look in force.

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

## How screens scale

Every pixel position, size and face height in this document is the value at
scale 1, on the C606's 240 × 320 panel. Other panels lay out from the same
rules at a scale step:

- The step is the smaller of the panel's width over 240 and its height over
  320, rounded down to a quarter and capped at 3/2. A panel under 3/4 cannot
  lay out. Faces and icons are generated only for the steps some device or
  simulator model uses, today 3/4 and 1, so a device whose panel lands on
  another step does not build until faces are generated for that step.
- A layout's lengths are units, which the step multiplies, rounded half up.
  A line of text takes the height of that step's own face.
- A step's face is drawn at the scaled size and advances by the scaled scale-1
  advance, both rounded down, so a line of text at a step is never wider than
  its scale-1 width scaled. A glyph whose ink fills its scale-1 advance keeps
  the one column more it needs, so letters never touch where they do not at
  scale 1. At a step, digits share the widest digit's advance.
- A screen splits a box into rows or columns: fixed units, the height of a
  face's line or capitals, and shares of what is left. A panel taller or wider
  than its step gives the extra space to the shares, so readings keep their
  size and gain room around them: the space over Home's start and the map
  grow, and lists show more cards.
- Every screen lays out from the panel and draws at the panel's size. Each
  device's build lays the screens out once for its panel, into one static
  layout, so a screen that does not fit a panel a device declares fails that
  device's build. `apps/vana/src/screens/mod.rs` holds that layout and
  `apps/vana/src/screens/scaled.rs` the steps.
- Shapes drawn at a fixed pixel size (the status bar's battery, bolt and dots,
  the gauges' rounded ends, a cue's disc and icon) keep that size at every
  step. Only where they sit scales.
- Touch is hit-tested in panel pixels against the same boxes the screen
  drew.
- A device names its panel in its `device.toml` under `devices/`, and
  `cycling devices` regenerates the list of panels a build lays out for,
  `apps/vana/src/screens/limits.rs`.

## Type scale

The typeface is Nunito (SIL Open Font License, `packages/shell/fonts/OFL.txt`).
Its rounded terminals are what makes the numerals read as VANA. Glyphs are
rasterized ahead of time into 4-bit coverage, with tabular digits so values do
not jitter as they change.

| Face | Nunito | Height | Use |
| --- | --- | --- | --- |
| `CAPTION` | 800, 13 px | 14 px, caps 11 | Units, labels, the status bar, chart annotations |
| `BODY` | 800, 18 px | 19 px, caps 14 | Card titles, toasts, sensor names, the clock |
| `TITLE` | 900, 34 px | 37 px, caps 26 | One headline per screen (RIDE, SENSORS) |
| `VALUE` | 700, 34 px | 29 px, digits 26 | Secondary readings: elapsed time, cadence, distance, grade |
| `LARGE` | 700, 50 px | 42 px, digits 37 | Primary readings beside others: power, heart rate |
| `HERO` | 700, 78 px | digits only | The one reading a page is about: speed on the main page of a ride without a course |

The course sign, the arrived totals and the map's footer have faces of their
own, each at the size and weight the mockups in `docs/design/course-navigation`
draw and holding only the characters it shows: `SIGN_50`, `SIGN_32`,
`SIGN_27`, `SIGN_20`, `SIGN_18`, `SIGN_14` and `SIGN_13` (Nunito 900) for the
sign, `TOTAL_60`, `TOTAL_30` (700) and `TOTAL_11` (900) for the totals, and
`FOOT_20` (700) for the distance in the map's footer. See Course sign below.

The kit has its own faces. `NOTE` (700, 13 px), `LABEL` (800, 15 px) and
`HEADING` (800, 21 px) hold printable ASCII, lowercase included, for
sentence-case subtitles and list labels, list values and button labels, and
a phone-style screen's title. `TAG` (800, 11 px, capitals) is for ride-page
field titles, zone tags and the top line's battery. `DIGITS_40`, `DIGITS_26`
and `DIGITS_15` (700) hold the value characters for ride-grid digits, the %
beside them and mid-size values, and the top line's elapsed time. A face
without lowercase draws a lowercase letter as its capital. `TAG`, `NOTE`,
`LABEL` and `HEADING` draw a middle dot in the caret's slot, and
`Face::glyph` reads a UTF-8 "·" there, so text writes "·" as it is; no
screen writes a caret.

`VALUE`, `LARGE` and `HERO` hold only ` +-./0123456789:%`. Units are a
`CAPTION` in `SECONDARY`, bottom-aligned with the number and 6 px to its
right. A missing reading shows `--` in the value's face and colour.

The 17 faces are listed in `packages/shell/fonts/generate.py`. To change a
face, edit that list and run `uv run packages/shell/fonts/generate.py`, which
rewrites `packages/shell/src/rendering/font/faces.rs`, then
`uv run packages/shell/fonts/generate.py --scale 0.75`, which rewrites
`packages/shell/src/rendering/font/faces_0_75.rs` with every face at the 3/4
step. Every face costs flash at each step it is generated for.

## Palette

Screens paint on `BLACK`. The light look (see Light below) maps each colour
as it is painted, so `BLACK` shows white; no screen paints a second palette.

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

At night VANA keeps the same palette. A dimmer palette would make the panel
darker still but would also shrink the gap between `WHITE` readings and `SECONDARY` labels, and
between `AMBER` and `RED` alerts and everything else, so it is not used:
alerts keep their full contrast at night.

### Brightness

`OPTION BRIGHTNESS` sets a fixed brightness, 10 to 100% in steps of 10; 100%
is the default. Dimming after inactivity keeps its own level on top of it.

### Light

`OPTION THEME` chooses NIGHT or DAY, the light look. NIGHT is the default.

The light look is derived from the one palette, applied to each colour as it
is painted. A screen paints through a row that carries the look, and the row
maps each palette colour once as it paints it. A colour as shown has a type
of its own, so a colour mapped twice or never does not compile. An
anti-aliased edge blends colours already mapped, so it ramps between the two
colours shown. The mapping, in `apps/vana/src/screens/look.rs`, inverts
luminance and keeps hue: `Y = (2r + 5g + b) / 8`, then each channel moves
by `255 - 2Y` and is clamped. Greys swap ends (`BLACK` becomes white,
`WHITE` becomes black). Bright colours such as `POWER` and `SPEED` turn
dark enough to read on white. Mid-bright colours such as `RED`, `HEART` and
`ACCENT` barely move under the inversion, so a saturated colour is also
pulled down towards a luma of 90, about 4:1 against white, in proportion to
its chroma. Greys keep the plain inversion. The brand frame at power on and
the power sheets take the look too. In dark mode the row maps nothing.

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
Without a profile or a reading the zone colour is `TERTIARY`. Heart rate uses
Z1 to Z5 of the same ramp.

Climb steepness uses the ramp from blue up, on the climb page's profile:

| Band | Colour |
| --- | --- |
| Descent, below -1.5% | `ZONES[1]` (blue) |
| Easy, -1.5% to 3% | `ZONES[2]` (green) |
| Moderate, 3–6% | `ZONES[3]` (yellow) |
| Steep, 6–9% | `ZONES[4]` (orange) |
| Hard, 9–12% | `ZONES[5]` (red) |
| Severe, 12% and more | `ZONES[6]` (purple) |

Each band starts at its lower bound and runs to the next band's. Along a
profile, a steeper grade takes its band at once, and a band drops back only
once the grade is 0.5% below where the band starts, so a steady grade on a
boundary keeps one colour.

## Shapes

| Shape | Rule |
| --- | --- |
| Card | Radius 18, `CARD` fill. When focused, `RAISED` fill with a 3 px `ACCENT` ring. |
| Small card or button | Radius 12, otherwise the same as a card |
| Pill (toasts, chips) | Radius = half its height |
| Gauge segment | 5 px tall, 3 px gaps, radius 2 |
| Dot | 6 px page dots; an 8 px tone dot in toasts |
| Icons | Anti-aliased, in one colour, blended over what lies behind like text. A sensor's icon takes its metric's colour where it names a saved sensor (`HEART`, `POWER`, `SPEED` for speed and speed/cadence, `CADENCE`) and `WHITE` for any other; Home's widget and the sheet draw a live one in its metric colour and one not yet live in `SECONDARY`, each with a status dot at the top-right, and NEARBY draws the sensors it lists in `SECONDARY`. The SVGs in `apps/vana/icons` are the source; see Icons below. |
| Ring | An arc of a ring with round ends, running clockwise from 12 o'clock. VANA's one shape for progress, charge and time left: the brand mark, the turn-off countdown, the charge and the power symbol. A ring that shows a share stands on a 25% track of its own colour. |

Shapes are anti-aliased coverage, blended over what lies behind them and
painted one display row at a time. Never draw a 1 px outline as decoration.

### Button cues

A screen that asks for a decision points at the buttons that answer it with a
cue: a quarter disc of radius 42 in the panel corner nearest the button, with
an icon and no words. The disc's colour says what the button does, so the cue
reads at a glance.

Each button in a device's `device.toml` names the side of the panel it sits
beside (`edge`: left, right, top or bottom) and how far along that side
(`at`, in thousandths from the side's top or left end). A cue takes the
corner at the nearer end of its button's side. A cue beside the middle of a
side is not designed yet, so `cycling devices` refuses a button with a role
in the middle third of its side (`at` 334 to 666). On the C606, BACK's cue
sits top left, MOVE's bottom left and ACT's bottom right. A device on which
two roles' buttons are nearest the same corner does not build.

| Icon | Disc | Meaning |
| --- | --- | --- |
| Play | `GREEN`, black icon | Start or resume (ACT) |
| Check | `GREEN`, black icon | Yes, or Done (ACT) |
| Stop | `RAISED`, white icon | End the ride: asks first (BACK) |
| Back | `RAISED`, white icon | No, go back (BACK) |
| Trash | `RED`, white icon | Throw a ride away or forget a sensor: beside MOVE it asks first, beside ACT it answers (or asks, on a sensor's page) |
| Target | `GREEN`, black icon | Zero a power meter (ACT) |

END RIDE? uses two of them at once: Back and Check. A boot's
FORMAT STORAGE? ([volumes](mmc-volumes.md#format)) puts Trash beside ACT.
A screen with a cue in a top corner shows without the status bar, which
would cut the cue. On the C606 that is every screen with a back cue: the
paused screen, END RIDE?, LOSE n UNSAVED RIDES?,
FORMAT STORAGE? and a sensor's pages. The
layout check fails any frame that draws a cue under the status bar.

Only the paused screen, End ride, LOSE n UNSAVED RIDES?,
FORMAT STORAGE? and a sensor's pages
(its detail and FORGET SENSOR?) show cues. Ride pages
never do: their buttons keep fixed roles. Content stays clear of the
corners the screen's own cues take on that device, wherever the buttons put
them. The layout check fails any frame that draws text or a shape under one
of its cues, and a test lays every cue screen out for buttons mirrored from
the C606's.

## Cells

The climb page (the map page draws instead) lists readings in cells: a `CAPTION` in
`SECONDARY` over its value in `VALUE`, in the metric's colour. Cells sit in two
columns from the body's 10 px margin, 48 px apart, so each caption has 7 clear
rows above it, and keep 8 px between the columns. A page that
cannot fit its readings at that rhythm shows fewer of them: space over
density. A value that does not fit its column steps down to
`BODY` on the same baseline. Captions are short enough for a column:
`DIST KM`, `AVG W`, `LAP BPM`, `KM/H DIFF`. A previous lap is named by its
number, `LAP 4`, as its toast named it.

A difference carries its sign: `GREEN` with "+" for a gain, `SECONDARY` with
"-" for a loss or no change. An easier lap is not a failure, so it is never
`RED`; red stays for warnings.

## Icons

Sensor icons are drawn on a 24-unit square with round caps and 2.4-unit
strokes, one SVG each in `apps/vana/icons`. To change one, edit its SVG and
run `uv run apps/vana/icons/generate.py`, which rewrites
`apps/vana/src/screens/icons/art.rs`. The script renders each whole icon at
every size a screen draws it, 22 px (Home and the sensor lists), 26 px (a
sensor's page) and 34 px, plus the 18 px refresh and plus marks and the
28 px replacement arrow, into 4-bit coverage cropped to the ink, about 7.8 KB
in all. It stops if two parts that do not touch in the drawing come within a
pixel of each other at any size; `--zoom DIR` writes 8x sheets to review.

## Sensors

The sensor screens are kit pages in both looks (`ScreenId::themed`). The
two lists are glance rows under the status bar; a sensor's page and
FORGET? are phone style.

- SENSORS lists the saved sensors under a back chevron and its `HEADING`
  title. A row is an icon in its metric's tone, the name in `LABEL`
  capitals over a `TAG` state line in its state's tone (LIVE in `good`,
  SEARCHING in `warn`, NOT IN RANGE in `sub`), and on the right the unit
  over the live reading in `DIGITS_26`, or `--` while the sensor sends
  nothing. A live sensor's line adds its voltage, or a battery that wants
  changing in its tone. A sensor the search just added says NEW. Two saved
  sensors of one kind take their maker's name. ADD SENSOR, in `good` with
  a plus, is the last row and opens NEARBY. A save or radio problem takes
  the title's place in `warn`.
- NEARBY is the only page that searches: it asks for a search as it opens,
  and leaving it stops the search. A `TAG` line under its title says how
  the search goes behind a dot (SEARCHING in `info`, or a problem in
  `warn`); a tap on it searches again. Its right end says WAKE A SENSOR
  until the search added one, then how many, such as 2 ADDED in `good`.
  Found sensors are listed strongest signal first with signal bars; each
  sensor added folds into a short row at the top that says LIVE with a
  check. The hint under the list says what to do next.
- Both lists are plain focus lists (`compose::tiles::sensor_list`): MOVE
  steps the focus, a tap selects, and the rows start at the first that
  lets the focused one show whole, at most five at once.
- A sensor's page has its top line (back chevron, SENSORS and the clock),
  its icon on a `raised` tile, its long name in `HEADING` and its maker in
  `NOTE`, then a card of Status, Reading, Battery, ID and Link. Forget sits
  over MOVE and, on a power meter, Zero over ACT; while a zero runs or just
  ran, Status says how it went. FORGET? asks with a trash disc, says which
  sensor, and puts Back over MOVE and Forget over ACT. Each button is the
  kit's `item`; a tap on one acts as its key.

## Spacing and chrome

A screen's boxes come from its tree. The chrome over them, at scale 1:

| Chrome | Rows | Notes |
| --- | --- | --- |
| `chrome::status_bar` | 0–21 | Ride mark (red dot and REC, or amber bars and PAUSED), page dots, then GPS, battery and clock on the right. No divider. A tree with a status tile places the bar itself. |
| `chrome::content` | 24–319 | Below the bar and the black under it. |
| `chrome::toast_band` | 280–319 | The toast pill sits at 282–311. Trees keep their least important reading, or nothing, here. |

The screen margin is 10 px. Text inside a card is inset 16 px. Cards are 8 px
apart.

Nothing is placed at a fixed x without a width budget. Text that can grow
(readings, clocks, summaries) is fitted to its box: VANA measures it, takes
the largest face from a short list that fits, and keeps its baseline.
If the smallest face still does not fit, drop the least important neighbour,
such as a caption, before cutting text. Each restyled screen has a sweep test
with extreme values: 120.0 km/h, 1999 W, 220 bpm, 180 rpm, 1000 km, 99:59:59
and full sensor lists. The test asserts that no two glyph runs overlap, that
nothing leaves its region and that every reading shows whole.

On ride pages, the radar strip takes the left 28 px only while a radar is
paired. Otherwise the body shifts 14 px to stay centred. The strip's track
ends above the toast band, so a toast never hides a radar target.

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
the clock, the battery, then the page dots and GPS. The ride mark drops its
word (REC, PAUSED, AUTO) where the word would reach the clock. Page dots
close up from an 11 px to an 8 px and then a 7 px pitch. If they still do
not fit, GPS gives way to them, and only then are they dropped. Any other
item that does not fit is left out. Items never overlap. While recording,
every combination fits.

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
2. An automatic resume (AUTO RESUME), shown briefly.
3. Transient announcements, shown for 2 s: RECORDING, RESUMED, RIDE SAVED
   (when a ride ends away from RIDE SAVED), RIDE RECOVERED. A pause has no
   toast: the paused screen says it. Battery news shows for 4 s, on any
   page: CHARGING 64% (`Good`) when USB power arrives, LOW BATTERY 20% (`Warn`) as the charge falls to
   20%, and CHARGE SOON 10% (`Alert`) at 10% and again at 5%. Each level is
   announced once per discharge; charging re-arms them. A device switched on
   already low says so only in its status bar. Ride alerts, chosen with
   `OPTION`, show here on ride pages, each with its cue
   ([sounds](ux-flows.md#sounds)): a limit broken for 10 s, HR HIGH 172, HR
   LOW 95, POWER HIGH 400W, POWER LOW 120W or CADENCE LOW 60, is `Warn` and
   repeats every 2 min while it lasts. EAT AND DRINK (`Info`) comes every
   chosen interval of ride time. ALL CLEAR (`Good`) follows 2 s after the
   last vehicle behind passed; the radar's beeps for vehicles have no toast.
   All are off by default except ALL CLEAR. Once per ride, BATTERY 40 MIN
   LEFT or BATTERY 2 H LEFT (`Warn`; hours and minutes round down) shows
   when the estimated time left has stayed for 5 min under 1.25 times the
   longer of one hour and the workout time still to ride. The estimate needs 6 points over 40 min,
   takes the worse of the 3 h median and the last 45 min, is cut by 30% under
   40% charge, and is silent when unknown, stale or above 95%. It restarts on
   charging, a rise of 2 steps that holds or a fall of over 20 points; a one
   point flicker is ignored and a smaller fall counts as one moment. A battery
   level toast keeps it back for 30 s. It rarely shows without a ride or an
   hour of use behind it, since nothing is stored between boots. Limits
   and reminders keep 3 s apart; the radar's beeps never wait.
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
   an automatic pause (the paused screen names it itself; [UX
   flows](ux-flows.md#paused) says which screen a pause opens on). A
   ride recording without wheel or GPS speed shows AUTO PAUSE OFF for its first 8 s
   of active time, not for the whole ride.

There are no bottom button labels. Each button has a fixed role, so the screen
does not need to explain them; where a screen asks a question, a button cue
points at the answer.

## Course sign

While a course is loaded, one sign tells the rider the next turn, in one of
three forms; [UX flows](ux-flows.md#course) says when each form shows and
what it says. Its geometry is the mockups' in
`docs/design/course-navigation`, laid out from their fractional positions
at the panel's scale step; `cycling mockup-compare` checks the data-page
screens against them. Every fill takes white ink, digits and units alike,
and the sign paints without the light look, so it is the same in both
looks. Its digits jump to each new distance: 10 m steps under 1 km, then
0.1 km, then whole km.

| Form | Where | Box | Content |
| --- | --- | --- | --- |
| Banner | Main and climb | Rows 22 to 104, full width, square corners | The glyph, then the distance in `SIGN_50` with its unit in `SIGN_20`; off course a second line TURN BACK in `SIGN_18`; arrived ARRIVED in `SIGN_27`; turn around the U-turn glyph and TURN over AROUND in `SIGN_27` on baselines 58 and 86 (`SIGN_20` on 56 and 78 in the badge) |
| Chip | Main, at rest | `RAISED` where the main page's tree places it, under the time; 28 tall, radius 8, as wide as its text; the glyph on a 28 × 28 turn-fill square | The distance in `SIGN_20`, its unit in `SIGN_13` `MUTED` |
| Badge | The map | At (8, 30), 60 tall, radius 12, as wide as its text | The banner at 60/82 of its size |

Fills are `#0a6fe0` for a turn, `#d67400` off course and turn around, and
`#1b9a45` arrived. A
unit sits 4 px after its digits' ink, and a fitted chip or badge ends 8.25 px
past its furthest ink, both in the mockups' fractional pixels; tests lock the
widths the mockups draw. With a radar paired the banner starts at x 28 and its
contents move 14 px right, as a data page's body does.

The main page is a tree of tiles (`screens/ride/main.rs`, #496): the status
bar, elapsed time in `VALUE` with grade in `CAPTION`, the chip on a course,
speed, power and heart rate in `LARGE` with their gauges, and the lower line
in `VALUE`. Its middle column grows no tile, so the room left over becomes
even gaps above, between and below the rows. Speed takes `HERO` where it fits
and no course is loaded, `LARGE` on a course. Without a radar the tree spans
the whole panel; beside one it re-tiles right of the strip. A reading's look
is measured on the widest text it can show, so a number never changes size
mid-ride; a lower-line reading that does not fit `VALUE` takes `BODY`. On a
course the time and the chip sit above the banner's bottom edge, which a test
holds on every panel. When main shows the ride's totals after arriving, the
banner's room replaces the time; the captions are `TOTAL_11`, distance
`TOTAL_60`, ride time `VALUE` and the three averages `TOTAL_30` at left,
centre and right. A toast may cover the lower line for a moment.

## Charts

Charts follow the same rules as the rest of the screen. VANA charts have no
axes, ticks, gridlines, legends or frames.

- **Baseline.** At most one 1 px `TERTIARY` baseline, and only when the zero
  matters (an elevation profile). Bars stand on black.
- **Colour encodes the category, height or length the quantity.** Use
  `ZONES` for zones and steepness, and the metric colour for a single metric.
  Never use a gradient that means nothing.
- **Past and future.** What has happened is drawn in full colour. What is
  ahead, or is only a reference, is drawn in that colour at 25% over black,
  or in `TERTIARY` when no colour applies.
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
| Elevation profile (climb) | A solid area per column in the look's zone colour for the steepness of that column's grade, under a 2 px outline in the look's text colour, with a 10 px disc for the rider in the text colour on a 2 px ring of the background. The road behind the rider mixes its colour half way to the background ([UX flows](ux-flows.md#climb) says which stretch the profile spans). Over it, LAST or CLIMB, AVG and MAX in `TAG`, each over its number in `DIGITS_19`. |
| Workout (workout page) | One bar per played step across the chart's box inside its gutters, with a 1 px gap. Its width is proportional to the step's planned time (distance and open steps get an estimate), its height to the target's intensity, scaled so the hardest step nearly fills the chart. A ramp is one stair per stage, up to the first stage at 150% of FTP (and always the stage on now), with width in proportion to its minutes and each stair's height and zone colour from its watts. The colour is the theme's zone colour of the target's middle, Z3's for cadence and the unlit colour without a target. Steps already ridden keep a third of their colour over the background; the step on now and those ahead are full colour. A 2 px line in the text colour runs the chart's height where the ride is now. |
| Target band (main page) | During a workout, a 2 px `WHITE` bar under the power or heart gauge spans the step's target. |
| Track (map) | See [Map page](#map-page) below. |

The time-in-zone bars and the profile reach into the toast band.
They are the least important part of their page, and a toast covers them for
a moment only.

### Map page

[UX flows](ux-flows.md#map) says what the map shows and when: heading up,
the scale, how far the course is drawn, and the rider's place on it. It
draws them with this geometry:

- **Ground.** The whole screen from the status bar to row 290, on ground
  `#101012`. With a radar paired the radar strip keeps columns 0 to 26 and
  the map's ground starts at x 28.
- **Puck.** Row 223, three quarters of the way down the map, and centred
  across it: x 120, or x 134 beside a radar. A 14.5 px black ring, a 12.5 px
  white disc and the `PUCK` arrow in `ACCENT` (`AMBER` off course,
  `SECONDARY` without a fix).
- **Route.** The course ahead is a 12 px `ACCENT` route on an 18 px
  `#04264c` casing with round caps and joins. Screen clipping hides what is
  off the map, and the way back from a turnaround past the top edge stops
  short of the puck.
- **Next turn.** The next turn's corner carries a 5 px white arrow on an
  8.5 px casing: 26 px of route from up to 10 px before the corner (a
  quarter of the way to the rider when nearer), then a head 10.5 px long and
  15.8 px wide.
- **Off course.** Up to three 3.5 px `AMBER` chevrons 15 px apart, pointing
  to the junction and centred between the rider and the junction, or where
  the straight line to it leaves the map, pushed out along that line until
  they clear the puck and stopping short of the junction's ring. The puck's
  row keeps them below the badge, and the map's left edge keeps them off a
  radar strip, whichever way the junction lies. The junction ring
  (`#101012` 12 px, `AMBER` 10 px, black 5.5 px, white 3.5 px) shows while
  it is on the map.
- **Before following starts.** Up to eight 2 px `#04264c` chevrons, 7 px
  wide and 3.5 px deep, sit on the course ahead every 25 m along it, counted
  from its end, or every second, fourth and so on of those, the fewest that
  keep them at least 40 px apart. They show from the rider's place on the
  course forward up to 320 px or the end of the drawn line, clear of the
  puck, each pointing the way the course runs. They stay put on the ground
  as the rider moves and as the scale eases, and thin out or fill in only
  where the scale doubles.
- **Badge.** The sign's badge sits at 8,30 over the map, at 36,30 with a
  radar paired, and may cover the route; it never reaches the puck.
- **Roads.** Under the course lie the terrain pack's roads: trunk, primary
  and secondary roads 9 px `#444448`, every other road 5 px `#2a2a2d`,
  square-ended, at those widths at every scale. Which roads are drawn
  follows the scale: every road up to about 530 m ahead, and further out no
  service roads, tracks or paths, switching at 560 m ahead going out and
  500 m coming in so speed noise holds the detail. Each road is simplified
  so no point strays half a pixel at the closest scale that draws it.
  Outside the pack, or without one, the course lies on the bare ground.
- **Own track.** The ride's own track, when the map draws it, is 5 px
  `#1f4a78`.
- **Footer.** The black footer, rows 290 to 320, keeps the whole width. It
  holds a 3 px `SECONDARY` scale bar from x 12, its length in `CAPTION`,
  and the distance in `FOOT_20`, `WHITE`, ending at x 203, with `KM` in
  `CAPTION` ending at x 228. The bar is the longest of 250 m, 100 m or 50 m
  that is at most 90 px (60 px for 100 m at 335 m ahead) and leaves its
  label 6 px clear of the distance. Under 10 km the distance leaves the
  whole 90 px; from 10 km the bar is at most 82 px, and from 1000 km at
  most 70 px. A bar that cannot fit at all is not drawn, nor its label.

The map paints its strokes as row spans: solid pixels are stored and only the
edges are blended. It draws the roads once per fill into a layer of the
ground that keeps two bits of coverage per road class and pixel, and each
frame paints the ground and roads from that layer by table.

## How data updates

- **Prepare once, paint per row.** On every frame, the screen turns the view
  into positioned labels, bars and shapes. Painting then fills one display
  row at a time: items whose rows it misses cost one comparison, and the
  rest paint spans. A glyph run blends over what the row already holds.
  Rounded shapes and discs fill solid parts as spans and work out only
  anti-aliased edges per pixel. Paint backgrounds first; later
  paints cover earlier ones. Each painted screen keeps its per-pixel original
  as a test reference, and tests assert whole frames match it.
- **Values change in place.** Digits are tabular and right- or left-aligned to
  fixed anchors, so a number never shifts its neighbours. A number jumps to
  each new reading; only graphics glide (see Motion).
- **Missing data looks dim, not alarming.** A stale or absent reading shows `--`.
  Zone colours fall back to `TERTIARY`. Errors belong in toasts or the status
  bar, not in the reading's place.
- **State changes announce themselves once.** Starting, resuming, laps and
  saving each raise a toast, and a pause shows the paused screen. Persistent
  conditions use the sticky toasts above or the status bar.

## Motion

Motion marks the device's own moments. Each is a ring that moves and text
that fades, prepared like any other frame, so a motion frame costs what a
still one does (about 25 ms on the C606). Motion frames draw every 40 ms; the
charging icon and sheets step every 500 ms; other pages look again once a
second. A page sends a frame only when what it shows changes, such as the
clock's minute, a reading, a glide that reaches the next pixel, the battery
or a sensor, or when a press or a test capture waits for a frame. A charging
battery that is almost full climbs by less than a pixel, so it sends nothing.

`OPTION DISPLAY` chooses NORMAL or SAVER, the rider's call, never
automatic. NORMAL is everything described here. SAVER draws less: gauges and
radar markers jump to each reading or report instead of gliding, radar
markers stop closing between reports, and every number on the ride pages changes
once a second, when the ride clock's second turns (the wall clock's while no
ride records): speed, power, heart rate, cadence, the distance and totals on
the lower line and the climb and workout figures. Gauges and markers still show every reading as it arrives, so
SAVER draws a frame for each report, about four a second, and one for the
numbers. `apps/vana/src/app/mode.rs` holds one table row per mode; the rest
of VANA reads its fields and never asks which mode is on. The choice is
stored in VANA's record, so it survives a restart.

Numbers never glide. A rider glancing down mid-glide could not read one, so
every number and word on the ride screen jumps to each reading as it
arrives, and the main page draws a new speed, power, heart rate or cadence
on the next frame. Only graphics glide. Sensors report about four times a
second; the power and heart-rate zone gauges ease from the drawn mark to
each new reading over 200 ms. They move fast at first, never pass the
reading and land on it exactly. Between reports, radar markers keep closing
at their reported speed for at most 300 ms, then glide to the next report.
A glide draws a frame only when it moves a gauge or marker onto another
pixel, at most every 40 ms. Only drawing glides: recording, totals and
alerts use readings as they arrive. The mechanism is
`packages/shell/src/rendering/glide.rs`; VANA's use of it is in
`apps/vana/src/app/motion.rs`.

| Moment | Motion | Duration |
| --- | --- | --- |
| Power on | VANA shows on the first lit frame; an `ACCENT` ring closes clockwise round it from 12 o'clock and LABS fades in. Home opens when it ends. | 400 ms, the brand's own time, so Home opens no later |
| Turn off | The ring's tail chases its head round until it is gone, while the wordmark and TURNING OFF fade to black. VANA asks the device to turn off once the black frame is on the panel. | 480 ms |
| Idle warning | An `AMBER` ring round the seconds empties with them. | 30 s |
| Charging while off | A `GREEN` ring shows the charge with its percentage inside, and a 50% green arc climbs from the charge to full as the status bar's icon does. CHARGED at 100%. The screen goes dark after 20 s; holding top left starts VANA as always. | 20 s |

## Sounds

Each cue's notes, and when a cue waits or is dropped, are in
[UX flows](ux-flows.md#sounds).

## Input

Three buttons control everything. Touch is a shortcut for navigation only.
What each button does on each screen, holding MOVE to mark a moment and
holding TopLeft to reboot are in [UX flows](ux-flows.md#buttons).

| Gesture | Rule | Effect |
| --- | --- | --- |
| Tap | Never moves 16 px from where it started, at any point; lifted within 600 ms | Acts on the control the finger landed on, wherever it lifts: runs the start (Ride, today's workout card, Just ride) or opens Sensors from the widget on Home, and the row on Sensors; a cue acts as its button. Rows own their bounds; the refresh mark's wider touch box takes only the space above the first row. On the workout page, a tap on the time left or its bar extends the step by a minute (500 m for a distance step) |
| Swipe | 10 px of mostly sideways travel, then at least 48 px and twice as far as vertically, within 1 s | Next page (right to left) or previous page (left to right) on ride pages |
| Long press | Still for 1 s, then lifted | On the map, stops following the course (COURSE OFF; [UX flows](ux-flows.md#map)) |
| Tap on the course sign | As a tap, on the banner or the chip | Opens the map page ([UX flows](ux-flows.md#course)) |

### How touch is routed

Touch follows UIKit's model. When a finger lands, the screen hit-tests what
lies under it: the control a tap would act on (a card, tile or row) and
whether the page takes swipes. The contact stays bound to that hit until it
lifts. The page's pull, the page's swipe and the control's tap then compete
for it, and the first to win takes it: 10 px of travel the way the page pulls
give it to the pull, as much mostly sideways travel to the swipe, and leaving
the 16 px tap slop any other way loses it for all. A control that lost is
released, never activated. `apps/vana/src/screens/touch.rs` owns this arbitration.

A control shows as pressed (`PRESSED`, iOS's touched-cell grey) as soon as
the finger lands on it.

Buttons go to the focus, which is not touch. A tap moves the focus only by
activating a control.

## Testing screens

The simulator reads the screen back as text, so scenarios assert what a rider
sees: `expect_text`, `field("W")`, `elapsed()`, `page()`. Anti-aliased text is
matched against glyph templates rendered from the same faces. The reader
tolerates glyphs that touch, as in `TT`. If a new layout makes two labels
ambiguous, rename one rather than weakening the reader.

`cycling sim` writes screenshots to `.local/sim/latest`; with `SIM_VIDEO=1` it
writes videos too.
`cycling sim-compare --pr <n>` renders the same scenarios on main and on a PR,
side by side; `cycling verify --pr <n>` runs it as its screens lane.

`cycling readme-screenshots` cuts the panel out of the checkpoints shown in the
README and writes them to `docs/screenshots`. Run it after changing one of those
screens.
