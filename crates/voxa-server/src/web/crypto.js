// Sign-in cryptography for the Voxa web admin page: X25519 (after TweetNaCl, public
// domain), SHA-256 and HMAC-SHA256. Written out here so signing in also works over plain
// HTTP, where browsers do not offer their built-in cryptography. The identity key never
// leaves the page: only a proof computed from it is sent.
'use strict';
(function (root) {
  // ---------------------------------------------------------------- X25519
  const gf = (init) => {
    const r = new Float64Array(16);
    if (init) for (let i = 0; i < init.length; i++) r[i] = init[i];
    return r;
  };
  const _121665 = gf([0xdb41, 1]);

  function car25519(o) {
    let c = 1;
    for (let i = 0; i < 16; i++) {
      const v = o[i] + c + 65535;
      c = Math.floor(v / 65536);
      o[i] = v - c * 65536;
    }
    o[0] += c - 1 + 37 * (c - 1);
  }

  function sel25519(p, q, b) {
    const c = ~(b - 1);
    for (let i = 0; i < 16; i++) {
      const t = c & (p[i] ^ q[i]);
      p[i] ^= t;
      q[i] ^= t;
    }
  }

  function pack25519(o, n) {
    const m = gf(), t = gf();
    for (let i = 0; i < 16; i++) t[i] = n[i];
    car25519(t);
    car25519(t);
    car25519(t);
    for (let j = 0; j < 2; j++) {
      m[0] = t[0] - 0xffed;
      for (let i = 1; i < 15; i++) {
        m[i] = t[i] - 0xffff - ((m[i - 1] >> 16) & 1);
        m[i - 1] &= 0xffff;
      }
      m[15] = t[15] - 0x7fff - ((m[14] >> 16) & 1);
      const b = (m[15] >> 16) & 1;
      m[14] &= 0xffff;
      sel25519(t, m, 1 - b);
    }
    for (let i = 0; i < 16; i++) {
      o[2 * i] = t[i] & 0xff;
      o[2 * i + 1] = t[i] >> 8;
    }
  }

  function unpack25519(o, n) {
    for (let i = 0; i < 16; i++) o[i] = n[2 * i] + (n[2 * i + 1] << 8);
    o[15] &= 0x7fff;
  }

  function A(o, a, b) { for (let i = 0; i < 16; i++) o[i] = a[i] + b[i]; }
  function Z(o, a, b) { for (let i = 0; i < 16; i++) o[i] = a[i] - b[i]; }
  function M(o, a, b) {
    const t = new Float64Array(31);
    for (let i = 0; i < 16; i++) for (let j = 0; j < 16; j++) t[i + j] += a[i] * b[j];
    for (let i = 0; i < 15; i++) t[i] += 38 * t[i + 16];
    for (let i = 0; i < 16; i++) o[i] = t[i];
    car25519(o);
    car25519(o);
  }
  function S(o, a) { M(o, a, a); }

  function inv25519(o, inp) {
    const c = gf();
    for (let a = 0; a < 16; a++) c[a] = inp[a];
    for (let a = 253; a >= 0; a--) {
      S(c, c);
      if (a !== 2 && a !== 4) M(c, c, inp);
    }
    for (let a = 0; a < 16; a++) o[a] = c[a];
  }

  function scalarMult(n, p) {
    const q = new Uint8Array(32);
    const z = new Uint8Array(32);
    const x = new Float64Array(80);
    const a = gf(), b = gf(), c = gf(), d = gf(), e = gf(), f = gf();
    for (let i = 0; i < 31; i++) z[i] = n[i];
    z[31] = (n[31] & 127) | 64;
    z[0] &= 248;
    unpack25519(x, p);
    for (let i = 0; i < 16; i++) {
      b[i] = x[i];
      d[i] = a[i] = c[i] = 0;
    }
    a[0] = d[0] = 1;
    for (let i = 254; i >= 0; --i) {
      const r = (z[i >>> 3] >>> (i & 7)) & 1;
      sel25519(a, b, r);
      sel25519(c, d, r);
      A(e, a, c);
      Z(a, a, c);
      A(c, b, d);
      Z(b, b, d);
      S(d, e);
      S(f, a);
      M(a, c, a);
      M(c, b, e);
      A(e, a, c);
      Z(a, a, c);
      S(b, a);
      Z(c, d, f);
      M(a, c, _121665);
      A(a, a, d);
      M(c, c, a);
      M(a, d, f);
      M(d, b, x);
      S(b, e);
      sel25519(a, b, r);
      sel25519(c, d, r);
    }
    for (let i = 0; i < 16; i++) {
      x[i + 16] = a[i];
      x[i + 32] = c[i];
      x[i + 48] = b[i];
      x[i + 64] = d[i];
    }
    const x32 = x.subarray(32);
    const x16 = x.subarray(16);
    inv25519(x32, x32);
    M(x16, x16, x32);
    pack25519(q, x16);
    return q;
  }

  const BASE = new Uint8Array(32);
  BASE[0] = 9;

  // ---------------------------------------------------------------- SHA-256
  // Round constants: the first 32 bits of the fractional parts of the cube roots (K) and
  // square roots (H) of the first primes.
  const primes = [];
  for (let n = 2; primes.length < 64; n++) if (primes.every((p) => n % p)) primes.push(n);
  const frac = (x) => Math.floor((x - Math.floor(x)) * 0x100000000) >>> 0;
  const K = new Uint32Array(primes.map((p) => frac(Math.cbrt(p))));
  const H0 = new Uint32Array(primes.slice(0, 8).map((p) => frac(Math.sqrt(p))));

  function sha256(data) {
    const H = new Uint32Array(H0);
    const l = data.length;
    const n = ((l + 9 + 63) >> 6) << 6;
    const m = new Uint8Array(n);
    m.set(data);
    m[l] = 0x80;
    const dv = new DataView(m.buffer);
    dv.setUint32(n - 4, (l * 8) >>> 0);
    dv.setUint32(n - 8, Math.floor(l / 0x20000000));
    const w = new Uint32Array(64);
    for (let off = 0; off < n; off += 64) {
      for (let i = 0; i < 16; i++) w[i] = dv.getUint32(off + i * 4);
      for (let i = 16; i < 64; i++) {
        const x = w[i - 15], y = w[i - 2];
        const s0 = ((x >>> 7) | (x << 25)) ^ ((x >>> 18) | (x << 14)) ^ (x >>> 3);
        const s1 = ((y >>> 17) | (y << 15)) ^ ((y >>> 19) | (y << 13)) ^ (y >>> 10);
        w[i] = (w[i - 16] + s0 + w[i - 7] + s1) >>> 0;
      }
      let a = H[0], b = H[1], c = H[2], d = H[3], e = H[4], f = H[5], g = H[6], h = H[7];
      for (let i = 0; i < 64; i++) {
        const S1 = ((e >>> 6) | (e << 26)) ^ ((e >>> 11) | (e << 21)) ^ ((e >>> 25) | (e << 7));
        const ch = (e & f) ^ (~e & g);
        const t1 = (h + S1 + ch + K[i] + w[i]) >>> 0;
        const S0 = ((a >>> 2) | (a << 30)) ^ ((a >>> 13) | (a << 19)) ^ ((a >>> 22) | (a << 10));
        const mj = (a & b) ^ (a & c) ^ (b & c);
        const t2 = (S0 + mj) >>> 0;
        h = g; g = f; f = e; e = (d + t1) >>> 0;
        d = c; c = b; b = a; a = (t1 + t2) >>> 0;
      }
      H[0] += a; H[1] += b; H[2] += c; H[3] += d;
      H[4] += e; H[5] += f; H[6] += g; H[7] += h;
    }
    const out = new Uint8Array(32);
    const ov = new DataView(out.buffer);
    for (let i = 0; i < 8; i++) ov.setUint32(i * 4, H[i]);
    return out;
  }

  function concat(...parts) {
    const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
    let o = 0;
    for (const p of parts) {
      out.set(p, o);
      o += p.length;
    }
    return out;
  }

  function hmacSha256(key, msg) {
    let k = key.length > 64 ? sha256(key) : key;
    const block = new Uint8Array(64);
    block.set(k);
    const ipad = block.map((b) => b ^ 0x36);
    const opad = block.map((b) => b ^ 0x5c);
    return sha256(concat(opad, sha256(concat(ipad, msg))));
  }

  // ---------------------------------------------------------------- encoding
  const utf8 = (s) => new TextEncoder().encode(s);
  function b64encode(bytes) {
    let s = '';
    for (const b of bytes) s += String.fromCharCode(b);
    return btoa(s);
  }
  function b64decode(s) {
    const bin = atob(s.trim());
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  }
  const hex = (b) => Array.from(b, (x) => x.toString(16).padStart(2, '0')).join('');
  const unhex = (s) => new Uint8Array(s.match(/../g).map((x) => parseInt(x, 16)));

  /// Known answers (RFC 6234 / 4231 / 7748): the page refuses to sign in if any fails.
  function selfTest() {
    const ok1 = hex(sha256(utf8('abc'))) === 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad';
    const ok2 = hex(hmacSha256(utf8('Jefe'), utf8('what do ya want for nothing?'))) === '5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843';
    const alice = unhex('77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a');
    const bobPub = unhex('de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f');
    const ok3 = hex(scalarMult(alice, BASE)) === '8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a';
    const ok4 = hex(scalarMult(alice, bobPub)) === '4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742';
    return ok1 && ok2 && ok3 && ok4;
  }

  root.VoxaCrypto = {
    publicKey: (priv) => scalarMult(priv, BASE),
    sharedKey: (priv, pub) => scalarMult(priv, pub),
    sha256,
    hmacSha256,
    concat,
    utf8,
    b64encode,
    b64decode,
    hex,
    selfTest,
  };
})(globalThis);
