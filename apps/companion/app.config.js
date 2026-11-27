// EAS reads only this file; Exact keeps app.json (exact docs, "Native CI preparation").
module.exports = {
  name: 'VANA',
  slug: 'vana-companion',
  owner: 'patrikduksin',
  version: process.env.EXACT_VERSION || '0.1.0',
  ios: { bundleIdentifier: 'computer.cycling.vana' },
  extra: { eas: { projectId: 'cff892b9-c6f9-4888-9704-3efb78f1297f' } },
};
