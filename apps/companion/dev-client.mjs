import { spawnSync } from 'node:child_process';
import { homedir } from 'node:os';
import { resolve } from 'node:path';

const LINK = 'https://appdrop.app/d/2aezfdx5bb';
const dir = import.meta.dir, ipa = resolve(dir, '.exact/dev.ipa');

const run = (cmd, args, options = {}) => {
  const r = spawnSync(cmd, args, { stdio: 'inherit', ...options });
  if (r.status !== 0) process.exit(r.status ?? 1);
  return r;
};
const tailscale = () => {
  const r = spawnSync('tailscale', ['ip', '-4'], { encoding: 'utf8' });
  const ip = r.stdout?.trim().split('\n')[0];
  if (r.status !== 0 || !ip) throw new Error('dev-client: no Tailscale address here; set DEV_URL to the dev server the phone opens');
  return `http://${ip}:8765/`;
};
const url = process.env.DEV_URL ?? tailscale();

// The cycling dev shell exports gcc as CC, which the Swift build hands its clang flags to.
const env = { ...process.env, EXACT_BUILD_NUMBER: String(Math.floor(Date.now() / 1000)) };
for (const key of ['CC', 'CXX', 'LD', 'AR', 'NIX_CC']) delete env[key];
env.PATH = `/run/current-system/sw/bin:${resolve(homedir(), '.cargo/bin')}:${env.PATH}`;

run(process.execPath, ['exact.mjs', 'ios', '--device', '--archive', ipa, '--unsigned', '--development', '--url', url], { cwd: dir, env });
// appdrop 0.11 or newer: its relay (`appdrop relay`, kept running on this machine) lets a phone
// with Tailscale on install. Older versions fail to install there.
run('bunx', ['appdrop@0.11.0', ipa, '--update', LINK], { cwd: homedir(), env });
console.log(`\ndev-client: published to ${LINK}\nOn the iPhone: update from the link, open the app, tap with four fingers → Open Project… → ${url} → Connect.`);
