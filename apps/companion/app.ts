import type { Answer, Result, Sources } from './app.contract.d.ts';

export const grants = 'device.bluetooth purpose.bluetooth';

type Native = { available: boolean; call(r: object): unknown; later(r: object): Promise<unknown>; watch(topic: string): void };
const needs = (native: Native | undefined): Native => {
  if (!native?.available) throw new Error('Bluetooth needs the iPhone app');
  return native;
};

const sources: Sources = {
  // The build renders the first screen without the module: no Bluetooth there.
  link: (_args, _store, _storage, native) => {
    const n = native as Native | undefined;
    if (!n?.available) return { state: 'idle', detail: '', name: '', psm: 0 };
    n.watch('vana.link');
    return n.call({ op: 'status' }) as Result<'link'>;
  },
  connect: async (_args, _store, _storage, native) => {
    const r = (await needs(native as Native | undefined).later({ op: 'connect' })) as { name: string; psm: number; ms: number };
    return { ok: true, text: `Open to ${r.name} on PSM ${r.psm} in ${r.ms} ms` };
  },
  echo: async ([bytes], _store, _storage, native) => {
    const r = (await needs(native as Native | undefined).later({ op: 'echo', bytes })) as
      { ok: boolean; bytes: number; ms: number; kbps: number; mismatches: number };
    return {
      ok: r.ok,
      text: `${Math.round(r.bytes / 1024)} KB echoed in ${(r.ms / 1000).toFixed(1)} s · ${r.kbps.toFixed(1)} KB/s both ways · ${r.mismatches} bad bytes`,
    };
  },
  download: async ([bytes], _store, _storage, native) => {
    const r = (await needs(native as Native | undefined).later({ op: 'download', bytes })) as
      { ok: boolean; bytes: number; ms: number; kbps: number; mismatches: number };
    return {
      ok: r.ok,
      text: `${Math.round(r.bytes / 1024)} KB downloaded in ${(r.ms / 1000).toFixed(1)} s · ${r.kbps.toFixed(1)} KB/s · ${r.mismatches} bad bytes`,
    };
  },
  disconnect: async (_args, _store, _storage, native) => {
    await needs(native as Native | undefined).later({ op: 'disconnect' });
    return { ok: true, text: 'Disconnected' };
  },
};
export const answer: Answer = (source, args, store, storage, native) =>
  sources[source](args, store, storage, native);
