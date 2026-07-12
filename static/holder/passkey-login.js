// Holder UI passkey sign-in island (Phase 2.b, chainlink #427).
//
// Drives the "Sign in with passkey" method on the holder login page: enter a
// handle → fetch an assertion challenge (allow-listing that holder's
// credentials) → navigator.credentials.get() → POST the assertion → redirect.
// WebAuthn is native (no vendored lib). This island is ALWAYS loaded on the
// login page, so it also owns the method-picker toggle (login-alpha.js is only
// present when login-α is enabled).
//
// The credential (de)serialization matches webauthn-rs-proto's wire shape:
// base64url strings on the wire ⇄ ArrayBuffers for the browser API.

// ---- base64url <-> ArrayBuffer helpers ----
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

// Convert an assertion PublicKeyCredential to webauthn-rs-proto's JSON shape.
function assertionToJson(cred) {
  return {
    id: cred.id,
    rawId: bufToB64url(cred.rawId),
    type: cred.type,
    response: {
      authenticatorData: bufToB64url(cred.response.authenticatorData),
      clientDataJSON: bufToB64url(cred.response.clientDataJSON),
      signature: bufToB64url(cred.response.signature),
      userHandle: cred.response.userHandle
        ? bufToB64url(cred.response.userHandle)
        : null,
    },
    clientExtensionResults: cred.getClientExtensionResults
      ? cred.getClientExtensionResults()
      : {},
  };
}

// Method picker: reveal the section matching the selected radio.
function wireMethodPicker() {
  const radios = document.querySelectorAll('input[name="login_method"]');
  if (radios.length === 0) return;
  const show = (method) => {
    document.querySelectorAll("[data-login-section]").forEach((el) => {
      el.hidden = el.getAttribute("data-login-section") !== method;
    });
  };
  radios.forEach((r) => r.addEventListener("change", () => show(r.value)));
  const checked = document.querySelector('input[name="login_method"]:checked');
  show(checked ? checked.value : "password");
}

function wirePasskeyLogin() {
  const form = document.getElementById("login-passkey-form");
  if (!form) return;
  const status = form.querySelector("[data-pk-status]");
  const setStatus = (m) => {
    if (status) status.textContent = m || "";
  };

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const identifier = form.querySelector("[data-pk-identifier]").value.trim();
    try {
      if (!identifier) throw new Error("Enter your handle.");
      setStatus("Starting…");

      // 1. Ask the server for an assertion challenge for this holder.
      const startRes = await fetch(
        "/oauth/atproto/holder/login/passkey/start",
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ identifier }),
        }
      );
      if (!startRes.ok) throw new Error("No passkey is registered for this account.");
      const { challenge_id, options } = await startRes.json();

      // 2. Decode the challenge + allow-list into ArrayBuffers for the browser.
      const pub = options.publicKey;
      pub.challenge = b64urlToBuf(pub.challenge);
      if (Array.isArray(pub.allowCredentials)) {
        for (const c of pub.allowCredentials) c.id = b64urlToBuf(c.id);
      }

      setStatus("Waiting for your passkey…");
      const cred = await navigator.credentials.get({ publicKey: pub });
      if (!cred) throw new Error("No credential was provided.");

      // 3. Submit the assertion; on success the server sets a session cookie.
      const finRes = await fetch(
        "/oauth/atproto/holder/login/passkey/finish",
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            challenge_id,
            credential: assertionToJson(cred),
          }),
        }
      );
      if (!finRes.ok) throw new Error("Passkey sign-in failed.");
      const { redirect } = await finRes.json();
      window.location = redirect || "/oauth/atproto/holder/home";
    } catch (err) {
      setStatus(err && err.message ? err.message : "Passkey sign-in failed.");
    }
  });
}

wireMethodPicker();
wirePasskeyLogin();
