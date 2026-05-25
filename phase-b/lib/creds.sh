# phase-b/lib/creds.sh — account seed + DID/JWT echo-confirm helpers.
#
# Source this file. Provides:
#
#   pb_create_account <role> <handle> <email> <password>
#       Seeds an account via dev.aurora.createAccount against role's PDS.
#       Echoes DID + JWT length AT THE SOURCE; exits non-zero if either
#       fails the well-formed checks. Sets two role-scoped exports:
#           <ROLE>_DID  — the created DID
#           <ROLE>_JWT  — the access JWT string
#       Plus an admin JWT (if mint-admin-token-on-create is available
#       in the deployment) on <ROLE>_ADMIN_JWT — left unset if not.
#
#   pb_grant_role <role> <did> <role-string>
#       Calls grant_role via dev tooling so the seeded account can act as
#       admin / moderator etc. (Not all scenarios need this; arc 9 does.)
#
# Why echo-confirm at the source: a JWT of literal length 4 (the string
# "null") cascades into auth failures several steps later that look
# unrelated. Catching the null at creation is the difference between
# "scenario failed on the third curl because login refused" and "creds
# block exited non-zero with did_was=did:plc:... jwt_len=4".
#
# Setup-only-never-judgment: this helper validates structure (DID
# well-formed prefix; JWT non-null length >= 250). It does NOT decide
# whether the seeded account is the right shape for a given scenario;
# that's the scenario block's prerogative.

set -u

# -----------------------------------------------------------------------------
# pb_create_account <role> <handle> <email> <password>
#
# Calls dev.aurora.createAccount on the role's PDS (port from
# <ROLE>_PORT). Captures the response, extracts DID + accessJwt,
# exports <ROLE>_DID + <ROLE>_JWT, and echoes both with a length check.
#
# The dev.aurora.createAccount endpoint is the seeding affordance from
# Arc 17's α-hosting decision; no extra dev-route needed.
# -----------------------------------------------------------------------------

pb_create_account() {
    local role="$1"
    local handle="$2"
    local email="$3"
    local password="$4"

    local upper
    upper=$(echo "$role" | tr '[:lower:]' '[:upper:]')
    local port_var="${upper}_PORT"
    local port="${!port_var:-}"

    if [ -z "$port" ]; then
        echo "[pb-creds] role=$role missing ${port_var}" >&2
        return 1
    fi

    local url="http://localhost:${port}/xrpc/dev.aurora.createAccount"
    local body
    body=$(jq -nc \
        --arg handle "$handle" \
        --arg email "$email" \
        --arg password "$password" \
        '{handle: $handle, email: $email, password: $password}')

    local resp
    resp=$(curl -sf -X POST "$url" \
        -H "Content-Type: application/json" \
        -d "$body" 2>&1) || {
        echo "[pb-creds] createAccount failed for role=$role handle=$handle:" >&2
        echo "$resp" >&2
        return 1
    }

    local did
    did=$(echo "$resp" | jq -r '.did // empty')
    local jwt
    jwt=$(echo "$resp" | jq -r '.accessJwt // empty')
    local admin_jwt
    admin_jwt=$(echo "$resp" | jq -r '.adminJwt // empty')

    # DID well-formed check.
    case "$did" in
    did:plc:*) : ;;
    *)
        echo "[pb-creds] DID malformed for role=$role: '$did' (expected did:plc:...)" >&2
        return 1
        ;;
    esac

    # JWT non-null length check. A literal "null" string is length 4;
    # bake the threshold (>= 250) the v0.5 markdown settled on.
    local jwt_len="${#jwt}"
    if [ "$jwt_len" -lt 250 ]; then
        echo "[pb-creds] accessJwt SHORT for role=$role: length=$jwt_len (expected >= 250)" >&2
        echo "[pb-creds] (length 4 = the string 'null'; check the response body for the real failure)" >&2
        return 1
    fi

    export "${upper}_DID=$did"
    export "${upper}_JWT=$jwt"
    if [ -n "$admin_jwt" ] && [ "$admin_jwt" != "null" ]; then
        export "${upper}_ADMIN_JWT=$admin_jwt"
    fi

    echo "[pb-creds] role=$role  did=$did  jwt_len=$jwt_len  admin_jwt_set=$( [ -n "${!upper}_ADMIN_JWT:-" ] && echo yes || echo no )"
}

# -----------------------------------------------------------------------------
# pb_echo_creds <role>
# Re-emit DID + JWT length for a role; useful at block-entry to confirm
# state survived across a terminal restart or sub-shell loss.
# -----------------------------------------------------------------------------

pb_echo_creds() {
    local role="$1"
    local upper
    upper=$(echo "$role" | tr '[:lower:]' '[:upper:]')
    local did_var="${upper}_DID"
    local jwt_var="${upper}_JWT"

    local did="${!did_var:-<unset>}"
    local jwt="${!jwt_var:-}"
    local jwt_len="${#jwt}"

    echo "[pb-creds] echo role=$role  did=$did  jwt_len=$jwt_len"
}

:
