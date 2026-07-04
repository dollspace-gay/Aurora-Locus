// Holder UI login-α island (Phase 2.a, chainlink #425).
//
// Drives the "sign in with your #atproto key" method on the holder login page.
// The holder pastes their secp256k1 private key (hex); this module fetches a
// server challenge, signs SHA-256(challenge) with the key, and submits the
// resulting did / nonce / signature to POST /oauth/atproto/holder/login. The
// backend verifies the signature against the holder's published
// identity_public_key (β.2's verify_login_signature) and mints a session.
//
// Security posture (Phase 2.a): the private key is paste-only — never persisted
// (no localStorage / IndexedDB), zeroed from the field after use, never sent to
// the server (the textarea has no `name`, so it cannot be form-submitted even if
// this module fails to load). Proper key-custody UX is a later cohort.

import * as secp from "./noble-secp256k1.js";

const enc = new TextEncoder();

function hexToBytes(hex) {
  const clean = hex.trim().replace(/^0x/i, "");
  if (clean.length === 0 || clean.length % 2 !== 0 || /[^0-9a-fA-F]/.test(clean)) {
    throw new Error("private key must be hex (even length)");
  }
  const out = new Uint8Array(clean.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}

function base64url(bytes) {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function qs(root, sel) {
  return root.querySelector(sel);
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

function wireLoginAlpha() {
  const form = document.getElementById("login-alpha-form");
  if (!form) return;
  const status = qs(form, "[data-la-status]");
  const setStatus = (msg) => {
    if (status) status.textContent = msg || "";
  };

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    setStatus("Signing…");
    const identifier = qs(form, "[data-la-identifier]").value.trim();
    const pkField = qs(form, "[data-la-privkey]");
    const privHex = pkField.value;
    try {
      if (!identifier) throw new Error("Enter your handle.");
      const priv = hexToBytes(privHex);

      // 1. Fetch a challenge for the resolved DID.
      const res = await fetch(
        "/oauth/atproto/holder/login/challenge?identifier=" +
          encodeURIComponent(identifier),
        { headers: { accept: "application/json" } }
      );
      if (!res.ok) throw new Error("No such did:web account, or key sign-in is off.");
      const { did, challenge } = await res.json();

      // 2. Sign SHA-256(challenge) with the #atproto key (low-S, 64-byte R‖S).
      const digest = new Uint8Array(
        await crypto.subtle.digest("SHA-256", enc.encode(challenge))
      );
      const sig = await secp.signAsync(digest, priv, { lowS: true });
      const compact = sig.toCompactRawBytes();

      // 3. Fill the hidden fields, zero the key, and submit natively.
      qs(form, "[data-la-did]").value = did;
      qs(form, "[data-la-nonce]").value = challenge;
      qs(form, "[data-la-signature]").value = base64url(compact);
      pkField.value = ""; // zero the key material out of the DOM
      // form.submit() (the method) does NOT re-fire the submit event, so this
      // does not recurse into this handler.
      form.submit();
    } catch (err) {
      pkField.value = ""; // never leave key material on a failure path
      setStatus(err && err.message ? err.message : "Could not sign in.");
    }
  });
}

wireMethodPicker();
wireLoginAlpha();
