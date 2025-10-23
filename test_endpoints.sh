#!/bin/bash

# Aurora Locus API Endpoint Tests
# Tests all Phase 2 account endpoints

BASE_URL="http://localhost:2583"

echo "========================================="
echo "Aurora Locus PDS - Endpoint Tests"
echo "========================================="
echo ""

# Test 1: Health Check
echo "Test 1: Health Check"
echo "GET $BASE_URL/health"
curl -s -X GET "$BASE_URL/health" | jq '.'
echo ""
echo ""

# Test 2: Server Description
echo "Test 2: Server Description"
echo "GET $BASE_URL/xrpc/com.atproto.server.describeServer"
curl -s -X GET "$BASE_URL/xrpc/com.atproto.server.describeServer" | jq '.'
echo ""
echo ""

# Test 3: Create Account
echo "Test 3: Create Account"
echo "POST $BASE_URL/xrpc/com.atproto.server.createAccount"
ACCOUNT_RESPONSE=$(curl -s -X POST "$BASE_URL/xrpc/com.atproto.server.createAccount" \
  -H "Content-Type: application/json" \
  -d '{
    "handle": "alice.localhost",
    "email": "alice@example.com",
    "password": "secure-password-123"
  }')
echo "$ACCOUNT_RESPONSE" | jq '.'

# Extract tokens for later use
ACCESS_JWT=$(echo "$ACCOUNT_RESPONSE" | jq -r '.accessJwt // .access_jwt')
REFRESH_JWT=$(echo "$ACCOUNT_RESPONSE" | jq -r '.refreshJwt // .refresh_jwt')
DID=$(echo "$ACCOUNT_RESPONSE" | jq -r '.did')

echo ""
echo "Extracted:"
echo "  DID: $DID"
echo "  Access Token: ${ACCESS_JWT:0:50}..."
echo "  Refresh Token: ${REFRESH_JWT:0:50}..."
echo ""
echo ""

# Test 4: Get Session (with auth)
echo "Test 4: Get Session Info"
echo "GET $BASE_URL/xrpc/com.atproto.server.getSession"
curl -s -X GET "$BASE_URL/xrpc/com.atproto.server.getSession" \
  -H "Authorization: Bearer $ACCESS_JWT" | jq '.'
echo ""
echo ""

# Test 5: Login (Create Session)
echo "Test 5: Login with Created Account"
echo "POST $BASE_URL/xrpc/com.atproto.server.createSession"
LOGIN_RESPONSE=$(curl -s -X POST "$BASE_URL/xrpc/com.atproto.server.createSession" \
  -H "Content-Type: application/json" \
  -d '{
    "identifier": "alice.localhost",
    "password": "secure-password-123"
  }')
echo "$LOGIN_RESPONSE" | jq '.'

# Extract new tokens
NEW_ACCESS_JWT=$(echo "$LOGIN_RESPONSE" | jq -r '.accessJwt // .access_jwt')
NEW_REFRESH_JWT=$(echo "$LOGIN_RESPONSE" | jq -r '.refreshJwt // .refresh_jwt')

echo ""
echo "New tokens after login:"
echo "  Access Token: ${NEW_ACCESS_JWT:0:50}..."
echo "  Refresh Token: ${NEW_REFRESH_JWT:0:50}..."
echo ""
echo ""

# Test 6: Refresh Session
echo "Test 6: Refresh Session Tokens"
echo "POST $BASE_URL/xrpc/com.atproto.server.refreshSession"
REFRESH_RESPONSE=$(curl -s -X POST "$BASE_URL/xrpc/com.atproto.server.refreshSession" \
  -H "Content-Type: application/json" \
  -d "{
    \"refreshJwt\": \"$NEW_REFRESH_JWT\"
  }")
echo "$REFRESH_RESPONSE" | jq '.'

REFRESHED_ACCESS_JWT=$(echo "$REFRESH_RESPONSE" | jq -r '.accessJwt // .access_jwt')

echo ""
echo "Refreshed access token: ${REFRESHED_ACCESS_JWT:0:50}..."
echo ""
echo ""

# Test 7: Logout (Delete Session)
echo "Test 7: Logout (Delete Session)"
echo "POST $BASE_URL/xrpc/com.atproto.server.deleteSession"
curl -s -X POST "$BASE_URL/xrpc/com.atproto.server.deleteSession" \
  -H "Authorization: Bearer $REFRESHED_ACCESS_JWT" | jq '.'
echo ""
echo ""

# Test 8: Try to use deleted session (should fail)
echo "Test 8: Try Using Deleted Session (Should Fail)"
echo "GET $BASE_URL/xrpc/com.atproto.server.getSession"
curl -s -X GET "$BASE_URL/xrpc/com.atproto.server.getSession" \
  -H "Authorization: Bearer $REFRESHED_ACCESS_JWT" | jq '.'
echo ""
echo ""

# Test 9: Create duplicate account (should fail)
echo "Test 9: Try Creating Duplicate Account (Should Fail)"
echo "POST $BASE_URL/xrpc/com.atproto.server.createAccount"
curl -s -X POST "$BASE_URL/xrpc/com.atproto.server.createAccount" \
  -H "Content-Type: application/json" \
  -d '{
    "handle": "alice.localhost",
    "email": "alice2@example.com",
    "password": "another-password"
  }' | jq '.'
echo ""
echo ""

# Test 10: Login with wrong password (should fail)
echo "Test 10: Login with Wrong Password (Should Fail)"
echo "POST $BASE_URL/xrpc/com.atproto.server.createSession"
curl -s -X POST "$BASE_URL/xrpc/com.atproto.server.createSession" \
  -H "Content-Type: application/json" \
  -d '{
    "identifier": "alice.localhost",
    "password": "wrong-password"
  }' | jq '.'
echo ""
echo ""

echo "========================================="
echo "All Tests Complete!"
echo "========================================="
