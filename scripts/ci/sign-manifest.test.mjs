// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { signature } from './sign-manifest.mjs';
// Public RFC 8032 test vector 1. This is not an application signing credential.
const seed = Buffer.from('9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60', 'hex');
const pub = Buffer.from('d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a', 'hex');
const expected = 'e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b';
test('signs the independent RFC vector with a seed or seed/public pair', () => {
  for (const key of [seed, Buffer.concat([seed, pub])]) assert.equal(Buffer.from(signature(Buffer.alloc(0), key.toString('base64'), pub.toString('base64')), 'base64').toString('hex'), expected);
});
test('refuses mismatched public keys and malformed credentials', () => {
  for (const key of ['', 'secret', seed.toString('base64') + '!', Buffer.alloc(31).toString('base64'), Buffer.concat([seed, Buffer.alloc(32)]).toString('base64')]) assert.throws(() => signature(Buffer.alloc(0), key, pub.toString('base64')));
  assert.throws(() => signature(Buffer.alloc(0), seed.toString('base64'), Buffer.alloc(32).toString('base64')));
});
