# 3. Multiple sensors of the same type

## Result

**The examined bridge cannot independently manage two active peers of one type.**
The limitation exists inside companion firmware as well as our current API.
Increasing the number of main-processor slots will not fix it.

## Evidence

N22 1.902 selects a fixed radio channel solely from device type at `0x1529c`.
The connect dispatcher at `0x13114` calls one profile instance per type. The
outgoing path uses the same lookup. There is no sender/channel selector in these
bridge requests that would address a second power meter, for example.

The receive forwarder at `0x132a8` has access to extended radio metadata but sends
only an eight-byte data payload under a type-number group. It omits both the
device number and radio channel. Discovery at `0x1c180` does expose the full
identity, but that information is not attached to each forwarded data page.

The link serializers at `0x16ce4` and `0x1be78` discard the high device-number byte
by shifting left before an eight-bit store. This confirms the existing workaround
is correlation with our selected peer, not independent full-identity verification.

There is another reason to retain strict replacement rules: profile connect paths,
including radar at `0x1bd68`, can emit a connected reply using the requested
identity when the profile is already marked connected, without opening a new
radio connection. An acknowledgment alone cannot prove that a replacement happened.

## Remaining uncertainty and decision

No alternative bridge route preserving per-packet identity was found in the
examined command and receive dispatchers. This is a limit of the recovered
interface, not a claim that ANT radio hardware cannot receive same-type peers.

**Keep one active sensor per type with this companion interface.** A saved library
can still contain multiple power meters or bikes, selecting one at a time after
confirmed disconnect. Concurrent same-type support would require a different
verified bridge path or a separately scoped companion-firmware project.
