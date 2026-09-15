# Device development tools

This Python package owns host access to firmware, private captures, and scenario
execution. Use the root `mise` tasks for routine operations. `mise` supplies the
source package path, so ordinary tools need no installation. Flash and backup
commands use the locked `uv` project to provide esptool. Bluetooth tools use the
system Python for the distribution's D-Bus and GLib bindings.

## Where behavior lives

- `device/` owns port discovery, protected flash operations, MMC inspection and
  maintenance. Flash safety checks stay beside the operation they protect.
- `terminal/` owns no-reset USB access, the device lock, command framing and logs.
- `rides/` owns export validation, decoding and explicit journal reclamation.
- `harness/` owns scenarios, real and virtual transports, camera and audio capture,
  and acquisition regression checks.
- `connectivity/` owns host Wi-Fi setup and Bluetooth test peers.
- `workspace.py` locates the checkout for commands and ignored private evidence.

Imports name the owning module. Package initializers do not re-export symbols.
Keep private outputs under the checkout's `.local/`; moving a tool must not change
its device lock or the location of existing backups and credentials.

## Run and test

From the repository root:

```sh
mise run terminal -- --help
mise run harness -- recipes
mise exec -- python -m cycling_devtools.harness.connectivity --help
mise exec -- python -m unittest discover -s tools/devtools/tests -p 'test_*.py'
```

The tests use fake transports and temporary files. They preserve coverage for
protected flash, uncertain writes, backup verification and ride export/reclaim.
Hardware scenarios need the device access rules in the C606 skill.
