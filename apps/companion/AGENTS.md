<!-- exact:begin (exact new writes this block; bun exact.mjs update rewrites it) -->
# vana-companion: an Exact app

Run `bun exact.mjs --help` first to create `.exact/checkout`; set `EXACT=<checkout>` if needed.

`app.contract` is the view, `data/src/lib.rs` the data module (Rust), `app.test.contract` the tests, `app.json`
the manifest: set `app.name` and `app.id` before building. The app uses Exact at
`.exact/checkout` by path (`EXACT` overrides it).

Its data module's guide is .exact/checkout/docs/reference.md, "Rust data sources". .exact/checkout/docs/start-here-site.md (a site) or
.exact/checkout/docs/start-here.md is the only required reading. `bun exact.mjs contract vocab <name>` answers a
name (none: the card); `bun exact.mjs docs <heading words>` a guide's
section. Run `bun exact.mjs verify` when the features work, and `verify ios|android` at the end for a
phone app. When an iPhone app is ready to try, run `bun exact.mjs share` and send its link;
repeat for each change the person should see. `bun exact.mjs help` lists the verbs, including
app.json's `"commands"`.
`bun exact.mjs update` refreshes files. Native verbs (`ios`, `mac`, `linux`, `android`, `windows`)
name their platform in app.json on first use and build in a generated workspace;
`update --without <platform>` stops shipping it.
`bun .exact/checkout/scripts/exact.mjs setup --check` names what a native build lacks (`--platform ios --device` for a phone's: `share`, `ios --device`).

Generated, so don't edit: `exact.mjs` and this block.

## Feedback (required)

When a doc was wrong or missing, a workaround was needed, something took several tries or a
command confused you, record it: `bun exact.mjs feedback note "<what happened>"`. It attaches
exact2's commit and the last exit code, never output or code. `bun exact.mjs feedback status` says
whether notes are sent and whether to keep a diary; under `never`, skip all of this.
<!-- exact:end -->

## Dev client on the iPhone

The phone runs a development build that follows the dev server on fleet-hub over Tailscale.
Proven 2026-11-25: a Contract edit appears on the phone in about two seconds.

1. The dev server is `bun exact.mjs web --lan --wasm` (the user service
   `vana-companion-dev` on fleet-hub). `--wasm` matters: a native client follows the wasm
   loop, and the default JS loop plus its resident wasm loop rebuild each other forever.
2. `bun exact.mjs dev-client` builds the development IPA and publishes it to VANA's
   AppDrop link, https://appdrop.app/d/2aezfdx5bb, then `systemctl --user restart
   vana-companion-dev` so the server admits the new binary. Needed after a change to
   `modules/apple/*.swift`, `data/`, grants or `app.json`. Contract edits reload live.
3. A phone with Tailscale on installs through `appdrop relay` (the `appdrop-relay` user
   service on fleet-hub, paired once with the phone; appdrop 0.11 or newer).
4. On the phone, open the app, tap with four fingers, choose Open Project…, enter
   `http://100.101.93.84:8765/` and choose Connect. Connection errors show in the dev pill.
   The app does not reconnect by itself after a relaunch.

`host.ios.appTransportSecurity.allowsArbitraryLoads` lets the dev build reach the server over
plain HTTP: `localNetworking` alone did not, for a Tailscale address. Take it out before a
release. `share` and Exact Build make production builds, which cannot follow the dev server.

## Release builds on Exact Build

`bun exact.mjs release` runs Exact's `share` on Exact Build (exact.appdrop.app): the server
compiles the Rust and Swift (about 2.5 min cold) and share publishes to
https://appdrop.app/d/ivktrs4mfc, keeping the link and build number in `.exact/appdrop.json`.
The token is `~/.exact-build-token` (Expo device login, 30 days); share prints the sign-in link
when it is missing. The release app has the dev client's bundle id, so installing one replaces
the other on the phone.
