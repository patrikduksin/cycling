# Course navigation: interaction spec

**One sign.** A turn instruction is one block: a glyph as tall as the block, the distance beside it, at most one line under it. Data, stats and climb pages show a full-width banner; the map shows the same block at 0.73 scale as a floating badge.

**Nav band.** Rows 22 to 104 belong to navigation. Readings below never move or resize. At rest it shows elapsed time, grade and a chip: white arrow on a blue square, then the next turn's distance. The banner covers only elapsed time and grade.

**Banner timing.** It appears when the turn is 30 s away, clamped to 150 to 400 m. It leaves 40 m past the turn, unless the next turn is inside its window.

**Dense turns.** Turns under 300 m or 25 s apart are dense; the sign stays up through the cluster. It always shows one turn, the next one, and switches as the rider passes each.

**States.** Ink is always white, units included; the fill carries the state.

- Turn: blue.
- Off course (40 m off for 5 s): deep amber `#d67400`, U-turn, distance back, TURN BACK.
- Arrived: deep green `#1b9a45`, flag. Totals replace live readings for 60 s; distance leads.

**Map badge.** 60 rows tall, top-left, 8 px from status bar and edge, as wide as its content.

**Map.** Heading-up, rider low. Zoom steps, never glides: 300 m, 400 m, 1 km or 3 km across. It takes the widest step showing the next turn; within 100 m, 300 m. It pans sideways so route, turns and rider clear the badge and the route ahead keeps a margin to the screen edge. Off course, amber chevrons lead to the missed junction.

**Sounds.**

- Banner appears: two rising beeps.
- 30 m before the turn: one beep.
- Off course: two falling tones, repeated after 15 s.
- Back on course: one rising tone.
- Arrival: the ride-start chime.

**Input.** Tap the banner or chip to open the map. Buttons keep their roles: ACT pauses, MOVE pages, BACK laps.
