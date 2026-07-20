import { argon2id } from 'hash-wasm';

const prefix = 'exo-encrypted:v1:';
type Envelope = { v: 1; kdf: 'argon2id'; m: number; t: number; p: number; s: string; n: string; ct: string };
const bytes = new TextEncoder();
const b64 = (v: Uint8Array) => btoa(String.fromCharCode(...v)).replaceAll('+', '-').replaceAll('/', '_').replaceAll('=', '');
const unb64 = (s: string) => Uint8Array.from(atob(s.replaceAll('-', '+').replaceAll('_', '/').padEnd(Math.ceil(s.length / 4) * 4, '=')), c => c.charCodeAt(0));
const aad = (id: string) => bytes.encode(`exo-encrypted:v1\0${id}`);
async function key(passphrase: string, salt: Uint8Array, e: Pick<Envelope, 'm'|'t'|'p'>) {
  return argon2id({ password: passphrase, salt, memorySize: e.m, iterations: e.t, parallelism: e.p, hashLength: 32, outputType: 'binary' }) as Promise<Uint8Array>;
}
export function isEncrypted(body: string) { return body.startsWith(prefix); }
export async function encryptBody(id: string, passphrase: string, body: string) {
  const salt = crypto.getRandomValues(new Uint8Array(16)); const nonce = crypto.getRandomValues(new Uint8Array(12));
  const e: Omit<Envelope, 'ct'> = { v: 1, kdf: 'argon2id', m: 65536, t: 3, p: 4, s: b64(salt), n: b64(nonce) };
  const k = await key(passphrase, salt, e); try { const ck = await crypto.subtle.importKey('raw', k, 'AES-GCM', false, ['encrypt']); const ct = new Uint8Array(await crypto.subtle.encrypt({ name: 'AES-GCM', iv: nonce, additionalData: aad(id), tagLength: 128 }, ck, bytes.encode(body))); return prefix + b64(bytes.encode(JSON.stringify({ ...e, ct: b64(ct) }))); } finally { k.fill(0); }
}
export async function decryptBody(id: string, passphrase: string, body: string) {
  if (!isEncrypted(body)) throw new Error('not encrypted'); const e = JSON.parse(new TextDecoder().decode(unb64(body.slice(prefix.length)))) as Envelope;
  if (e.v !== 1 || e.kdf !== 'argon2id') throw new Error('unsupported encrypted note'); const k = await key(passphrase, unb64(e.s), e);
  try { const ck = await crypto.subtle.importKey('raw', k, 'AES-GCM', false, ['decrypt']); return new TextDecoder().decode(await crypto.subtle.decrypt({ name: 'AES-GCM', iv: unb64(e.n), additionalData: aad(id), tagLength: 128 }, ck, unb64(e.ct))); } finally { k.fill(0); }
}
