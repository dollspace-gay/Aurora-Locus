#!/bin/bash

# Comprehensive Sequencer Endpoints Test Script
# Tests all 6 sequencer management endpoints with proper authentication

BASE_URL="http://localhost:2583"

echo "================================="
echo "SEQUENCER ENDPOINTS TEST SUITE"
echo "================================="
echo ""

# Step 1: Create admin account
echo "Step 1: Creating admin account..."
ACCOUNT_OUTPUT=$(curl -s -X POST "$BASE_URL/xrpc/com.atproto.server.createAccount" \
  -H "Content-Type: application/json" \
  -d '{
    "email": "testuser@localhost",
    "handle": "alice.localhost",
    "password": "TestPassword123!"
  }')

echo "Account creation response: $ACCOUNT_OUTPUT"
echo ""

# Extract access token
ACCESS_TOKEN=$(echo "$ACCOUNT_OUTPUT" | grep -oP '"accessJwt":"\K[^"]+' | head -1)
DID=$(echo "$ACCOUNT_OUTPUT" | grep -oP '"did":"\K[^"]+' | head -1)

if [ -z "$ACCESS_TOKEN" ]; then
  echo "❌ Failed to get access token. Account may already exist."
  echo "Trying to create a session with existing account..."

  # Try to create session
  SESSION_OUTPUT=$(curl -s -X POST "$BASE_URL/xrpc/com.atproto.server.createSession" \
    -H "Content-Type: application/json" \
    -d '{
      "identifier": "alice.localhost",
      "password": "TestPassword123!"
    }')

  echo "Session response: $SESSION_OUTPUT"
  ACCESS_TOKEN=$(echo "$SESSION_OUTPUT" | grep -oP '"accessJwt":"\K[^"]+' | head -1)
  DID=$(echo "$SESSION_OUTPUT" | grep -oP '"did":"\K[^"]+' | head -1)
fi

if [ -z "$ACCESS_TOKEN" ]; then
  echo "❌ Failed to authenticate. Cannot proceed with tests."
  exit 1
fi

echo "✓ Successfully authenticated"
echo "  DID: $DID"
echo "  Token: ${ACCESS_TOKEN:0:50}..."
echo ""

# Step 1.5: Grant admin role to the account
echo "Step 1.5: Granting admin role..."
python3 -c "
import sqlite3
conn = sqlite3.connect('data/account.sqlite')
cursor = conn.cursor()
# Insert admin role for this DID
cursor.execute('''
  INSERT OR IGNORE INTO admin_roles (did, role, granted_by, granted_at, revoked)
  VALUES (?, 'admin', 'system', datetime('now'), 0)
''', ('$DID',))
conn.commit()
conn.close()
print('✓ Admin role granted to $DID')
"
echo ""

# Step 2: Test all 6 sequencer endpoints
echo "================================="
echo "TESTING SEQUENCER ENDPOINTS"
echo "================================="
echo ""

# Test 1: getSequencerStatus
echo "Test 1: GET getSequencerStatus"
echo "------------------------------"
RESPONSE=$(curl -s -X GET "$BASE_URL/xrpc/com.atproto.admin.getSequencerStatus" \
  -H "Authorization: Bearer $ACCESS_TOKEN")
echo "$RESPONSE" | python3 -m json.tool 2>/dev/null || echo "$RESPONSE"
echo ""

# Test 2: listRecentEvents
echo "Test 2: GET listRecentEvents"
echo "----------------------------"
RESPONSE=$(curl -s -X GET "$BASE_URL/xrpc/com.atproto.admin.listRecentEvents?limit=5" \
  -H "Authorization: Bearer $ACCESS_TOKEN")
echo "$RESPONSE" | python3 -m json.tool 2>/dev/null || echo "$RESPONSE"
echo ""

# Test 3: pauseSequencer
echo "Test 3: POST pauseSequencer"
echo "---------------------------"
RESPONSE=$(curl -s -X POST "$BASE_URL/xrpc/com.atproto.admin.pauseSequencer" \
  -H "Authorization: Bearer $ACCESS_TOKEN" \
  -H "Content-Type: application/json")
echo "$RESPONSE" | python3 -m json.tool 2>/dev/null || echo "$RESPONSE"
echo ""

# Test 4: Check status after pause
echo "Test 4: Verify paused status"
echo "----------------------------"
RESPONSE=$(curl -s -X GET "$BASE_URL/xrpc/com.atproto.admin.getSequencerStatus" \
  -H "Authorization: Bearer $ACCESS_TOKEN")
echo "$RESPONSE" | python3 -m json.tool 2>/dev/null || echo "$RESPONSE"
echo ""

# Test 5: resumeSequencer
echo "Test 5: POST resumeSequencer"
echo "----------------------------"
RESPONSE=$(curl -s -X POST "$BASE_URL/xrpc/com.atproto.admin.resumeSequencer" \
  -H "Authorization: Bearer $ACCESS_TOKEN" \
  -H "Content-Type: application/json")
echo "$RESPONSE" | python3 -m json.tool 2>/dev/null || echo "$RESPONSE"
echo ""

# Test 6: resetSequencerCursor
echo "Test 6: POST resetSequencerCursor"
echo "---------------------------------"
RESPONSE=$(curl -s -X POST "$BASE_URL/xrpc/com.atproto.admin.resetSequencerCursor" \
  -H "Authorization: Bearer $ACCESS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"target_seq": 0}')
echo "$RESPONSE" | python3 -m json.tool 2>/dev/null || echo "$RESPONSE"
echo ""

# Test 7: rebuildSequencer (verify only)
echo "Test 7: POST rebuildSequencer (verify_only)"
echo "-------------------------------------------"
RESPONSE=$(curl -s -X POST "$BASE_URL/xrpc/com.atproto.admin.rebuildSequencer" \
  -H "Authorization: Bearer $ACCESS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"verify_only": true}')
echo "$RESPONSE" | python3 -m json.tool 2>/dev/null || echo "$RESPONSE"
echo ""

echo "================================="
echo "ALL TESTS COMPLETED"
echo "================================="
