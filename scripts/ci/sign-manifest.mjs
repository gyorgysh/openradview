// SPDX-License-Identifier: GPL-3.0-or-later
// Signing only: never exports a key or downloads an SDK. The environment secret
// must match the public key already compiled into the application.
import { createPrivateKey, createPublicKey, sign, timingSafeEqual } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

export function signature(raw, encoded, expected) {
  const secret = Buffer.from(encoded.trim(), 'base64');
  try {
    if (![32, 64].includes(secret.length) || secret.toString('base64') !== encoded.trim()) throw Error('Invalid signing key encoding');
    const der = Buffer.concat([Buffer.from('302e020100300506032b657004220420', 'hex'), secret.subarray(0, 32)]);
    try {
      const key = createPrivateKey({ key: der, format: 'der', type: 'pkcs8' });
      const publicKey = createPublicKey(key).export({ format: 'der', type: 'spki' }).subarray(-32);
      const configured = Buffer.from(expected, 'base64');
      if (configured.length !== 32 || !timingSafeEqual(publicKey, configured) ||
          (secret.length === 64 && !timingSafeEqual(publicKey, secret.subarray(32)))) throw Error('Signing key does not match the app public key');
      return sign(null, raw, key).toString('base64');
    } finally { der.fill(0); }
  } finally { secret.fill(0); }
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    if (process.argv.length !== 4 || !process.env.UPDATER_ED25519_PRIVATE_KEY) throw Error('Missing signing inputs');
    const config = JSON.parse(readFileSync(new URL('../../packaging/updater.json', import.meta.url)));
    const result = signature(readFileSync(process.argv[2]), process.env.UPDATER_ED25519_PRIVATE_KEY, config.public_ed25519_key);
    writeFileSync(process.argv[3], result + '\n', { flag: 'wx', mode: 0o600 });
  } catch { console.error('Update signing failed; check the protected environment key and output path.'); process.exitCode = 1; }
  finally { delete process.env.UPDATER_ED25519_PRIVATE_KEY; }
}
