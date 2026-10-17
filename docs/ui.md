# VANA UI

VANA has one visual language. A rider glances at a 240 × 320 screen for under a
second, often in sunlight, while moving. So every screen follows the same rules:
black space instead of boxes, a few large numbers, and colour that always means
something. This document defines those rules. The code that implements them
lives in `apps/vana/src/screens/chrome.rs` (geometry, palette, shapes, status
bar, toasts) and `packages/shell/src/rendering/font` (type).

Pages that are not yet in this language still use the 5 × 7 bitmap font: the
stats, lap and climb ride pages, Settings and Preflight. They should move to it
using the rules below and nothing else.

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

The current climb page still uses its older pure RGB565 colours. The restyle
should replace them with these.

## Shapes

| Shape | Rule |
| --- | --- |
| Card | Radius 18, `CARD` fill. When focused, `RAISED` fill with a 3 px `ACCENT` ring (`chrome::card`). |
| Small card or button | Radius 12, otherwise the same as a card |
| Pill (toasts, chips) | Radius = half its height |
| Gauge segment | 5 px tall, 3 px gaps, radius 2 |
| Dot | 6 px page dots; an 8 px tone dot in toasts |
| Icons | Drawn in one colour on a card. A state badge (a green check) sits at the top-right. |

Shapes are coverage functions (`disc`, `rounded`), blended over what lies
behind them, and painted per row by `paint_rounded` and `paint_disc`. Never
draw a 1 px outline as decoration.

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
  and AUTO);
- which page is showing;
- GPS: grey while searching, green when fixed;
- battery: white, amber at 30% or less; below 20% a red percentage replaces
  the icon;
- the clock.

Items are laid out with measured widths, in priority order: the ride mark,
the clock, the battery, GPS, then page dots. Dots close up from an 11 px to an
8 px pitch before they are dropped, and an item that still does not fit is
left out. Items never overlap. While recording, every combination fits.

Messages appear as toasts: a `RAISED` pill with a tone dot and `BODY` text,
centred over the bottom of the content.

| Tone | Dot | Used for |
| --- | --- | --- |
| `Good` | Green | Saved, resumed |
| `Info` | Blue | Laps, recovery, neutral news |
| `Warn` | Amber | An armed press (PRESS AGAIN TO SAVE), paused, low storage |
| `Alert` | Red | Storage failures |

Only one toast shows at a time. From highest priority to lowest:

1. The armed END RIDE confirmation.
2. The lap just taken.
3. An automatic start or resume (AUTO START, AUTO RESUME), shown briefly.
4. Transient announcements, shown for 2 s: RECORDING, RESUMED, PAUSED, RIDE
   SAVED, RIDE RECOVERED. An automatic pause skips PAUSED, because item 7
   names it.
5. On ride pages only, sticky storage faults: STORAGE ERROR, then STORAGE
   FULL, WRITE FAILED, STORAGE NOT READY. A storage write failure is never
   hidden: the only one item 6 may replace is the bulk failure that moved
   the ride to internal flash. Any later failure shows here.
6. Sticky low storage: under one hour left, the toast shows `REC 42M LEFT`.
   Otherwise, while bulk media cannot take rides and the ride page shows an
   internal ride, a `Warn` toast names it: READY INT, RECORDING INT, PAUSED
   INT, AUTO PAUSED INT, SAVED INT or RECOVERED INT. After a recovery, RIDE
   RECOVERED stays until recording resumes.
7. Auto-pause: AUTO PAUSED stays for as long as an automatic pause lasts. A
   ride recording without wheel speed shows AUTO PAUSE OFF for its first 8 s
   of active time, not for the whole ride.

There are no bottom button labels. Each button has a fixed role, so the screen
does not need to explain them.

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
| Time in zone (stats) | One 8 px bar per metric, segments proportional to time in each zone, in zone colours, with 2 px gaps and no empty zones. Below it, a `CAPTION` with the most-ridden zone and its time. |
| Elevation profile (climb) | A solid area per column, coloured by the steepness of that column's grade. The climbed part is full colour; the rest of a known climb is 25%. A `WHITE` disc marks the rider. One `TERTIARY` baseline. A `CAPTION` with gain and length sits at the right end. |
| Lap comparison (laps) | One vertical bar per lap, 8 px wide with 3 px gaps, height proportional to the lap's average. The metric colour comes from the page's metric (power, speed or time). Finished laps are drawn at 25%, the current lap in full colour, and the best lap gets a `WHITE` cap. The newest lap is on the right; older laps scroll off the left. |

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
  fixed anchors, so a number never shifts its neighbours. There is no
  animation. The next frame is the update.
- **Missing data looks dim, not alarming.** A stale or absent reading shows `--`.
  Zone colours fall back to `TERTIARY`. Errors belong in toasts or the status
  bar, not in the reading's place.
- **State changes announce themselves once.** Starting, pausing, resuming,
  laps and saving each raise a toast. Persistent conditions use the sticky
  toasts above or the status bar.

## Input

Three buttons control everything. Touch is a shortcut for navigation only.

| Button | Home | Sensors | Settings | Ride |
| --- | --- | --- | --- | --- |
| BottomRight (ACT) | Open the focused card | Act on the focused tile | Edit the field; while editing, raise it | Start, pause, resume |
| BottomLeft (MOVE) | Focus the next card | Focus the next tile | Next field; while editing, lower it | Next page (wraps) |
| TopLeft (BACK / MARK) | Turn off, after the TURN OFF sheet confirms | Back to the tiles, then Home | Home; while editing, finish | LAP while recording; END RIDE while paused (press again within 3 s); Home when no ride is open |

| Gesture | Rule | Effect |
| --- | --- | --- |
| Tap | Never moves 16 px from where it started, at any point; lifted within 600 ms | Opens the card or tile under the finger on Home and Sensors. Tiles own their bounds; the search button's wider touch margin takes only spare space |
| Swipe | Moves at least 48 px horizontally and at least twice as far as vertically, within 1 s | Next page (right to left) or previous page (left to right) on ride pages |
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
