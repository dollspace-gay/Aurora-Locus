// Holder UI passkey registration island (Phase 2.b, chainlink #427).
//
// Drives the "Add passkey" action on the auth-methods page: request a creation
// challenge → navigator.credentials.create() → POST the attestation. The holder
// is authenticated (browser session), so both requests carry the session CSRF
// token. WebAuthn is native (no vendored lib). The credential (de)serialization
// matches webauthn-rs-proto's wire shape (base64url ⇄ ArrayBuffer).

function b64urlToBuf(s) {
  const pad = "=".repeat((4 - (s.length % 4)) % 4);
  const b64 = (s + pad).replace(/-/g, "+").replace(/_/g, "/");
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out.buffer;
}
function bufToB64url(buf) {
  const bytes = new Uint8Array(buf);
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

// Convert an attestation PublicKeyCredential to webauthn-rs-proto's JSON shape.
function attestationToJson(cred) {
  return {
    id: cred.id,
    rawId: bufToB64url(cred.rawId),
    type: cred.type,
    response: {
      attestationObject: bufToB64url(cred.response.attestationObject),
      clientDataJSON: bufToB64url(cred.response.clientDataJSON),
    },
    clientExtensionResults: cred.getClientExtensionResults
      ? cred.getClientExtensionResults()
      : {},
  };
}

function wirePasskeyRegister() {
  const form = document.getElementById("add-passkey-form");
  if (!form) return;
  const status = form.querySelector("[data-pk-status]");
  const setStatus = (m) => {
    if (status) status.textContent = m || "";
  };
  const csrf = form.querySelector('input[name="csrf_token"]').value;

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const deviceName = form.querySelector("[data-pk-device-name]").value.trim();
    try {
      setStatus("Starting…");

      // 1. Ask the server for a creation challenge.
      const startRes = await fetch(
        "/oauth/atproto/holder/auth-methods/passkey/start",
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ csrf_token: csrf }),
        }
      );
      if (!startRes.ok) throw new Error("Could not start passkey registration.");
      const { challenge_id, options } = await startRes.json();

      // 2. Decode base64url fields into ArrayBuffers for the browser API.
      const pub = options.publicKey;
      pub.challenge = b64urlToBuf(pub.challenge);
      pub.user.id = b64urlToBuf(pub.user.id);
      if (Array.isArray(pub.excludeCredentials)) {
        for (const c of pub.excludeCredentials) c.id = b64urlToBuf(c.id);
      }

      setStatus("Waiting for your passkey…");
      const cred = await navigator.credentials.create({ publicKey: pub });
      if (!cred) throw new Error("No credential was created.");

      // 3. Submit the attestation; the server verifies + stores it.
      const finRes = await fetch(
        "/oauth/atproto/holder/auth-methods/passkey/finish",
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            csrf_token: csrf,
            challenge_id,
            credential: attestationToJson(cred),
            device_name: deviceName || null,
          }),
        }
      );
      if (!finRes.ok) throw new Error("Passkey registration failed.");
      window.location.reload();
    } catch (err) {
      setStatus(err && err.message ? err.message : "Passkey registration failed.");
    }
  });
}

wirePasskeyRegister();
