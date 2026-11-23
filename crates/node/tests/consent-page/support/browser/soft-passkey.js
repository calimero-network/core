// A software ES256 passkey for the app-origin page: Chromium refuses rp id `localhost` from a
// `<label>.localhost` origin, so the app page signs in with this and the consent page uses the real one.
window.SoftApp = function (pk, origin, counter) {
  const enc = new TextEncoder();
  const b64 = (u) =>
    btoa(String.fromCharCode(...u))
      .replace(/\+/g, '-')
      .replace(/\//g, '_')
      .replace(/=+$/, '');
  const unb64 = (s) => Uint8Array.from(atob(s.replace(/-/g, '+').replace(/_/g, '/')), (c) => c.charCodeAt(0));
  const toDer = (sig) => {
    const int = (x) => {
      let i = 0;
      while (i < x.length - 1 && x[i] === 0) i++;
      const t = [...x.slice(i)];
      if (t[0] & 0x80) t.unshift(0);
      return [0x02, t.length, ...t];
    };
    const body = [...int(sig.slice(0, 32)), ...int(sig.slice(32))];
    return new Uint8Array([0x30, body.length, ...body]);
  };
  // WebCrypto signs P1363; the DER conversion above is what WebAuthn wants.
  let key;
  const keyP = crypto.subtle
    .importKey('pkcs8', unb64(pk.pkcs8), { name: 'ECDSA', namedCurve: 'P-256' }, false, ['sign'])
    .then((k) => (key = k));
  return {
    async get(challenge) {
      await keyP;
      const rpHash = new Uint8Array(await crypto.subtle.digest('SHA-256', enc.encode(pk.rpId)));
      const authData = new Uint8Array([
        ...rpHash,
        0x05,
        (counter >>> 24) & 255,
        (counter >>> 16) & 255,
        (counter >>> 8) & 255,
        counter & 255,
      ]);
      const clientData = enc.encode(
        JSON.stringify({ type: 'webauthn.get', challenge: b64(challenge), origin, crossOrigin: false }),
      );
      const ch = new Uint8Array(await crypto.subtle.digest('SHA-256', clientData));
      const sig = new Uint8Array(
        await crypto.subtle.sign(
          { name: 'ECDSA', hash: 'SHA-256' },
          key,
          new Uint8Array([...authData, ...ch]),
        ),
      );
      return {
        credentialId: pk.credentialId,
        authenticatorData: b64(authData),
        clientDataJson: b64(clientData),
        signature: b64(toDer(sig)),
      };
    },
    async create() {
      throw new Error('software app passkey cannot register');
    },
  };
};
