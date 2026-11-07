# Companion recovery: primary sources and limits

Research date: 2026-11-06. Scope: the original Magene C606's nRF52810
companion, particularly an application that fails before UART handling or
ESP32 power control works. This was desk research only: no device access,
probe execution, firmware changes, or external messages. Downloads and source
extracts are confined to ignored `.local/recovery-sources/`.

**Finding: there is no established closed-case recovery procedure for that
failure.** The nRF52810 has no factory ROM UART recovery loader. Nordic's
software bootloaders can provide recovery independently of an application,
but the installed Magene bootloader, its entry policy, its power initialization,
and its reset wiring are unavailable. A healthy application's update command
does not establish entry when that application cannot execute.

The follow-up [bootloader acquisition investigation](companion-bootloader-search.md)
scanned the available backup/package artifacts and traced the stock application
UART command surface. Neither route yielded the missing bootloader binary.

## Evidence and source access

Public hardware and SDK claims below use Nordic's own source distribution and
MDK. Magene claims use its product page, linked manual, and support API. Vendor
behavior recovered in earlier local analysis is explicitly identified as local
evidence, not public vendor documentation or a newly verified bootloader fact.

Nordic's Product Specification and DevZone pages returned a Cloudflare challenge
in this research environment. They were not read successfully and are not cited
as if inspected. The accessible Nordic MDK register descriptions, startup code,
and official SDK archive provide the primary evidence used here. In particular,
this report does not claim to have checked the complete hardware reset-retention
matrix in the Product Specification.

## What Magene actually documents

- The original [C606 product page][M1] links the [C606 English manual][M2].
  Its English section, PDF pages 2–7 (printed pages 1–6), covers installation,
  charging, OnelapFit activation, specifications, warranty, and battery warnings.
  It does not give a companion bootloader-entry sequence, UART rescue procedure,
  or a guarantee about recovery from replacement companion firmware. The product
  page mentions firmware V1.9 features, which establishes ordinary update support,
  not recovery entry without working application firmware.
- The original [C606 support category][M3] currently has only a product overview
  and an installation-video article. Its FAQ and video section listings were
  checked through Magene's public API. Searches for `C606`, `firmware`, `update`,
  `upgrade`, `reset`, `restart`, `crash`, `frozen`, `button`, `DFU`, and
  `bootloader` found no original-C606 companion recovery instructions. The last
  two terms returned no articles. This is a bounded search result, not proof
  that Magene has no private service procedure.
- Magene does document holding the **two lower buttons for 20 seconds until a
  beep**, then holding the top button to power on, in [a freeze/black-screen
  article][M4]. **That article belongs to C606 Pro**, established by its
  section/category metadata. [Similar instructions][M5] belong to C606 V2.
  Neither article identifies which processor resets, says the buttons directly
  assert nRF nRESET, or promises companion DFU entry. They cannot establish an
  application-independent recovery route for the original C606.
- The [firmware-update instructions found in a GPS troubleshooting article][M6]
  require connecting the bike computer to OnelapFit and using the normal update
  UI. That article also belongs to C606 Pro. It provides no broken-companion
  bypass. Other search results for factory reset concerned C406/C406 Pro;
  resetting user settings is not evidence of a companion flash recovery loader.

## Hardware and SDK mechanisms

| Mechanism | What primary sources establish | What it cannot guarantee on this C606 |
| --- | --- | --- |
| Factory ROM UART loader | **None on nRF52810.** Nordic's [memory layout and startup][N1] put executable startup/vectors in programmable flash. Its [SDK MBR interface][N3] explicitly reserves flash for the MBR and locates bootloader addresses in flash/UICR. UART DFU is supplied by installed bootloader software [N2]. | There is no ESP32-like factory serial-download fallback to invoke by a strap, reset, or UART break. An installed MBR/SoftDevice is flash software, not an immutable ROM UART rescue loader. |
| GPREGRET | An 8-bit general-purpose retention register at POWER offset `0x51c` [N4]. Nordic SDK 17.1.0 defines DFU-start value `0xB1`, checks it only when GPREGRET entry is enabled, and clears the entry flag after accepting DFU [N2]. | Writing a magic number has no hardware DFU effect. Some executing code must set it, reach a reset/handoff, and an intact bootloader must honor it. An ESP32 UART command cannot set an nRF register if its receiving code never runs. It is not nonvolatile storage or a flag guaranteed to survive loss of nRF power. |
| Watchdog | nRF52810 has a WDT, a watchdog-reset reason, configurable behavior during CPU sleep/halt, and a reload mechanism [N4]. SDK bootloader code can feed an already-running WDT during DFU [N2]. | A crash before WDT startup has no WDT escape. Feeding from a surviving task can hide other failures; a sleep-paused WDT can fail to expire. Even a genuine watchdog reset only restarts the boot chain: Nordic's example DFU-entry check has **no watchdog-reset condition**. An image that validates but crashes can loop forever. |
| Physical buttons | A bootloader can independently sample a GPIO at startup. Nordic's example does this under `NRF_BL_DFU_ENTER_METHOD_BUTTON` [N2]. | Application button handlers cannot rescue an application that never reaches them. A held button only helps if something first resets the nRF and the installed bootloader samples that connected pin or independently handles buttons while running. Nordic's development-kit example pin is not C606 wiring. No vendor source establishes a hidden combination, double reset, or crash counter. |
| P0.21 / nRESET | Nordic [startup code][N5] selects pin 21 for nRF52810 when configuring pin reset; two UICR `PSELRESET` entries control connection. The [register description][N4] defaults these entries to disconnected. | P0.21 is not automatically an externally usable reset input on every board. Its programmed configuration and wiring must be established. Resetting it still does not force DFU unless the bootloader implements that policy. No closed-case access or ESP32 connection to this pin is established. |
| UART break | Nordic describes break as an `ERRORSRC.BREAK` condition in UART/UARTE [N4]. Its SDK serial DFU implementation initializes UART and parses SLIP packets after DFU initialization [N2]. | Break is not a silicon reset or ROM-loader entry mechanism. A running application or bootloader could choose to interpret it, but that is software-specific. The generic SDK UART error handler takes an error path, not a universal DFU-entry path. Sending breaks to Magene's unknown implementation could merely produce errors or resets into the same failing image. |
| USB reconnect / ESP32 restart | These can restart or reconnect the main processor and its USB interface. Existing C606 evidence assigns companion communication to an ESP32 UART [L1], [L2]. | USB enumeration and an ESP32 `reset=power` report say nothing conclusive about nRF reset. The nRF can remain alive while cycling the ESP32's power/enable. An ESP reset does not set the nRF's GPREGRET or assert its nRESET without an established extra mechanism. |

The no-ROM conclusion concerns **factory recovery over UART**, not the absence
of hardware debugging. SWD is a separate mechanism. The board-photo desk notes
identify internal SWD pads, with accessibility requiring opening the case;
debug protection is unknown. This is outside the owner's closed-case constraint.

### Generic Nordic behavior is not Magene bootloader evidence

The inspected **nRF5 SDK 17.1.0** files in the [official archive][N2] show:

- `components/libraries/bootloader/nrf_bootloader.c`, `dfu_enter_check()`:
  invalid application, configured button, configured pin-reset reason,
  configured GPREGRET flag, or configured buttonless settings flag can select
  DFU. `app_is_valid()` checks stored image metadata and boot validation;
  it does not execute the application to determine whether it works.
- `nrf_bootloader_info.h`: `BOOTLOADER_DFU_START` is `0xB0 | 0x01`.
  `dfu_enter_flags_clear()` in the preceding file consumes the flag. A successful
  DFU handoff once does not leave an everlasting recovery request.
- `examples/dfu/secure_bootloader/pca10040_uart/config/sdk_config.h`: that
  **nRF52832 development-kit example**, not an nRF52810/C606 configuration,
  enables button entry on P0.16 and GPREGRET entry, disables pin-reset entry,
  and sets a 120-second DFU inactivity timeout. Even the generic example does
  not promise that a pin reset always enters DFU or that DFU waits forever.
- `serial_dfu/nrf_dfu_serial_uart.c`: UART DFU is an initialized software
  transport with configurable pins/flow control and SLIP framing.
  `nrf_bootloader_wdt.c` feeds a WDT if already started; it does not implement
  a watchdog-failure recovery policy.

These are examples of possible implementations, **not a fingerprint of the
installed loader**. The locally recovered Magene update protocol uses its own
framing/pages. Similar GPREGRET magic does not prove identical Nordic entry
options, transport, validation, inactivity timeout, or reset policy.

## What the existing C606 evidence supports

The read-only private notes `.local/motion-wake/board-desk.md`,
`companion-desk.md`, and `main-desk.md` in the owner's primary checkout were
inspected; no reconstruction/disassembly helpers were executed. These are
unpublished analysis of vendor artifacts, not the unavailable bootloader itself.
Their sanitized, relevant conclusions are:

1. **DFU entry currently depends on the companion application.** Local analysis
   routes group-one page `0x24`, byte 1 equal to zero, through the app scheduler
   into a GPREGRET `0xB1` handoff. The byte-1-equals-one mode query reports app
   or bootloader mode. There is no established independent UART command path
   when the app's UART/scheduler is dead [L1], [L2].
2. **The application handoff is a jump, not a hardware reset.** The earlier
   main desk note called it a system reset. Subsequent direct decoding of the
   checksum-identified complete N22 1.902 image resolved the discrepancy: it
   writes GPREGRET, tears down peripherals, loads the initial stack pointer
   and entry point from the MBR vectors, restores CPU control/mask registers,
   loads MSP and branches to that entry point. This sequence still executes
   inside the running application; it does not demonstrate an independent
   entry trigger. See the local verification below.
3. **Stock main firmware has a conditional repair path.** Local analysis says
   that a bootloader-mode reply can trigger sending a staged companion update.
   This requires an intact responding loader, a suitable staged image, the
   main updater running, and main power. It does not establish how an
   unresponsive, boot-valid application is forced into that state. Staged-file
   availability and independent ESP32 power are not guaranteed [L2].
4. **Power direction matters.** The desk notes assign ESP32 power/enable to a
   companion control output. Nordic GPIO23/GPIO13 belong to the GNSS path,
   correcting older power hypotheses [L2]. No ESP32-to-nRF reset/power-control
   line is established. Absence from static analysis is not proof that no
   unrecognized wire exists. Board photographs do not establish that USB
   powers the ESP32 independently of a nonfunctional companion.
5. **Buttons work through stock application logic in the recovered code.**
   The companion note identifies button GPIOs P0.14, P0.25, and P0.20 and
   application event/state handling. Earlier suspend recovery restored UART
   after a physical button press [L1]. That is evidence for a live stock
   application's button route, not bootloader-owned entry or hardwired reset.

Consequently, the private notes' categorical statements that a bootloader-stuck
companion is repairable must be read with the prerequisites above. Their advice
to preserve UART, DFU handling, ESP power, and the watchdog in custom firmware
is useful engineering guidance; it does not prove recovery from a fault before
those safeguards execute. Pre-arming GPREGRET in a future app would likewise
need vendor-loader validation and cannot cover a fault before that write or
loss of the host's power.

## Why the recent update is not broken-app recovery proof

The [same-version update observation][L2] starts with a **healthy stock app**.
Entry and image transfer progressed; the last captured packet was **602/614**.
Near completion, USB disappeared, the ESP32 later reported a power reset, and a
new mode query obtained a **fresh app-mode reply** with fresh companion sensor
traffic. This strongly supports a functioning stock update path followed by
return to normal operation. The final packet acknowledgment and uploader
Complete state were not captured; there was no companion flash readback.

Objections to a stronger recovery claim:

- The initial DFU request was serviced by the working app. The failure under
  discussion removes that very entry mechanism.
- No deliberate early application crash, boot-valid but nonfunctional image,
  or interrupted/invalid-image recovery was tested. Image validity and runtime
  health are different properties in Nordic's example [N2].
- A same-version app-mode reply cannot identify which bytes were installed;
  fresh sensor reports establish current execution, not full transfer readback
  or the missing final acknowledgment.
- The ESP32 power cycle does not establish that the nRF reset, that P0.21 was
  asserted, or that a button/GPREGRET/WDT condition can independently select DFU.
- Power surviving the **healthy-app handoff** does not establish that the
  bootloader will enable the ESP32 after a cold nRF start with a crashing app.
- Acceptance of the original stock image does not establish acceptance of an
  unsigned or modified image, its validation rules, or an available rollback.

Recovery from an image rejected by boot validation is also distinct from
recovery from an image that passes validation but crashes before its UART and
power initialization. The latter is the unresolved closed-case risk.

## Direct stock-code verification

After the documentation review, selected routines were independently decoded
from the complete recovered N22 1.902 application using Capstone's Thumb/M-class
mode. The decoded image was checked against the previously identified SHA-256
`e7e46db0b2f0f5be7a9fe4b10ef9996e15870f148fb4923df4833b9ee4dde6e5`.
Raw bytes, instruction listings and analysis helpers remain in ignored
`.local/recovery-analysis/`; this section publishes conclusions only.

- **The button-combination handler is application code.** An application
  timer callback calls it, and it consumes the application's decoded button
  events. One branch powers the ESP32 control low, raises the ESP32 boot
  strap, waits, and restores ESP32 power. Its log identifies ESP programming
  mode. This is an ESP32 download-mode route mediated by the healthy nRF
  application, not a hardware nRF bootloader-entry mechanism.
- **The combination's reset path also relies on application execution.**
  A later threshold schedules an application timer whose callback invokes
  the Cortex-M system-reset routine. Neither that callback nor the separately
  traced reset call in the button handler sets GPREGRET to request DFU.
  This can reset a working nRF application; its behavior after reset still
  depends on the unavailable bootloader. There is no evidence that this
  combination would work if application event processing never runs.
- **The DFU handler has a different path.** Its scheduled application
  callback writes the DFU flag and jumps through the MBR vectors as described
  above. The presence of the other reset routine is not evidence that DFU
  performs a hardware reset, nor that a button can force the DFU flag.
- **The complete control table resolves a power-pin uncertainty.** Its six
  indexed entries map controls 0–5 to Nordic P0.6, P0.12, P0.10, P0.11,
  P0.27 and P0.7. Thus stock's ESP32 power/enable control 5 selects P0.7.
  The ESP boot strap selects P0.24. These are code-derived pin assignments;
  their electrical wiring was not measured. They establish nRF-to-ESP
  control, not an independently usable ESP-to-nRF reset line.
- **The available image is still only the application.** Its decoded flash
  range is `0x12000` through `0x252a8` exclusive. Earlier private notes
  incorrectly subtracted the container overhead a second time and reported
  `0x25226`. Neither the lower MBR/SoftDevice region nor a separate upper
  bootloader is included, so these findings cannot establish the loader's
  button/reset entry policy or validation behavior.

**Test decision:** no independently controlled recovery trigger with understood
behavior was identified. No device commands, button experiments, resets, UART
breaks, interrupted transfers or new firmware writes were performed in this
investigation. Repeating an ordinary main restart would add liveness evidence
without testing the failure at issue, so it was not used as recovery proof.
The existing device firmware was left unchanged. The following observation
proposal remains optional and would not remove the central uncertainty.

## Plausible low-risk observation, not executed

With the existing healthy stock companion left unchanged, a future owner-guided
session could passively capture ordinary startup traffic and send only the
already-established **read-only mode query** before and after an ordinary
ESP32 restart. Use the existing serialized device workflow; record main reset
reason, fresh query replies and independently advancing sensor reports. Do not
send DFU-entry/file/data pages, UART break, arbitrary GPIO commands, or replace
the companion app. This is a proposal, not authorization or an executed test.

This can characterize main-restart behavior and, if it happens to be observed,
a responding bootloader window. An app-mode reply proves only current app
liveness; silence does not distinguish a missed window, power loss, or a dead
app. **It cannot prove recovery from an early nRF application crash**, because
an ESP32 restart need not reset the nRF. Deliberately corrupting the app or
interrupting its update is not a low-risk substitute while the case cannot be
opened and bootloader behavior is unknown.

Evidence needed to remove the uncertainty: the installed vendor bootloader and
its configuration/UICR state, a matching schematic establishing reset/power
paths, or an explicit Magene procedure for this exact original-C606 companion
failure. No vendor contact was made. No universal closed-case button sequence
or UART rescue command can responsibly be supplied from the sources inspected.

## Primary-source links

[M1]: https://www.magene.com/en/all-products/85-c606-gps-smart-bike-computer.html
[M2]: https://static.rfsvr.com/prestashop/attachment/20240115/1705316296_C606_EN.pdf
[M3]: https://support.magene.com/hc/en-us/categories/27747447500441-C606-Smart-GPS-Bike-Computer
[M4]: https://support.magene.com/hc/en-us/articles/55676723821081-Touchscreen-failure-black-screen-system-crash-no-response-when-charging-or-powering-on
[M5]: https://support.magene.com/hc/en-us/articles/56183385342745--The-computer-won-t-turn-on-freezes-after-booting
[M6]: https://support.magene.com/hc/en-us/articles/55723381521689-GPS-positioning-abnormality-or-failure-to-locate
[N1]: https://github.com/nordicsemi/nrfx/blob/v3.2.0/mdk/nrf52810_xxaa.ld
[N2]: https://developer.nordicsemi.com/nRF5_SDK/nRF5_SDK_v17.x.x/nRF5_SDK_17.1.0_ddde560.zip
[N3]: https://developer.nordicsemi.com/nRF5_SDK/nRF5_SDK_v17.x.x/nRF5_SDK_17.1.0_ddde560.zip
[N4]: https://github.com/nordicsemi/nrfx/blob/v3.2.0/mdk/nrf52810.svd
[N5]: https://github.com/nordicsemi/nrfx/blob/v3.2.0/mdk/system_nrf52.c
[L1]: foundation-research.md
[L2]: companion-update.md

- Magene article search API:
  <https://support.magene.com/api/v2/help_center/articles/search.json?query=C606&per_page=100>.
- Original C606 section listings:
  <https://support.magene.com/api/v2/help_center/sections/27747492370073/articles.json?per_page=100>
  and <https://support.magene.com/api/v2/help_center/sections/27747516157593/articles.json?per_page=100>.
- Article M4's section metadata identifies category `55540453098265`:
  <https://support.magene.com/api/v2/help_center/sections/55540484231833.json>.
  Category names can be checked at
  <https://support.magene.com/api/v2/help_center/categories.json?per_page=100>.
- Nordic startup vectors (alongside the memory layout N1):
  <https://github.com/nordicsemi/nrfx/blob/v3.2.0/mdk/gcc_startup_nrf52810.S>.
- Nordic register bitfield definitions (alongside N4):
  <https://github.com/nordicsemi/nrfx/blob/v3.2.0/mdk/nrf52810_bitfields.h>.
- N3 is the archive member
  `components/softdevice/mbr/headers/nrf_mbr.h`, particularly `MBR_SIZE`,
  `MBR_BOOTLOADER_ADDR`, and `MBR_UICR_BOOTLOADER_ADDR`.
- Official SDK archive SHA-256:
  `5bfe38e744c39fd7f30e10077ba12df306ef91f368894795d6a3e7a62dc68061`.
  This identifies the public SDK download, not any vendor firmware.

Public links and SDK source paths identify the evidence without publishing
vendor firmware, disassembly, board-specific identifiers, or raw captures.
