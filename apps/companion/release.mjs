import { spawnSync } from 'node:child_process';

const env = {
  ...process.env,
  EXACT_EXPERIMENTAL: '1',
  EXACT_BUILD: '1',
  // 0.11 or newer installs on a phone with Tailscale on, through `appdrop relay`.
  EXACT_APPDROP_CLI: 'appdrop@0.11.0',
};
const r = spawnSync(process.execPath, ['exact.mjs', 'share', ...process.argv.slice(2)], { cwd: import.meta.dir, env, stdio: 'inherit' });
process.exit(r.status ?? 1);
