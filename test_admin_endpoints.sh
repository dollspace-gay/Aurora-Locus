#!/bin/bash

# Get access token
SESSION_OUTPUT=$(curl -s -X POST "http://localhost:2583/xrpc/com.atproto.server.createSession" -H "Content-Type: application/json" -d '{"identifier": "alice.localhost", "password": "TestPassword123!"}')
ACCESS_TOKEN=$(echo "$SESSION_OUTPUT" | grep -oP '"accessJwt":"\K[^"]+' | head -1)

echo "=== SEQUENCER ENDPOINT TESTS ==="
echo "Access token: ${ACCESS_TOKEN:0:50}..."
echo ""

echo "Test 1: getSequencerStatus"
curl -s -X GET "http://localhost:2583/xrpc/com.atproto.admin.getSequencerStatus" -H "Authorization: Bearer $ACCESS_TOKEN"
echo ""
echo ""

echo "Test 2: listRecentEvents"
curl -s -X GET "http://localhost:2583/xrpc/com.atproto.admin.listRecentEvents?limit=5" -H "Authorization: Bearer $ACCESS_TOKEN"
echo ""
echo ""

echo "Test 3: pauseSequencer"
curl -s -X POST "http://localhost:2583/xrpc/com.atproto.admin.pauseSequencer" -H "Authorization: Bearer $ACCESS_TOKEN" -H "Content-Type: application/json"
echo ""
echo ""

echo "Test 4: getSequencerStatus (should be paused)"
curl -s -X GET "http://localhost:2583/xrpc/com.atproto.admin.getSequencerStatus" -H "Authorization: Bearer $ACCESS_TOKEN"
echo ""
echo ""

echo "Test 5: resumeSequencer"
curl -s -X POST "http://localhost:2583/xrpc/com.atproto.admin.resumeSequencer" -H "Authorization: Bearer $ACCESS_TOKEN" -H "Content-Type: application/json"
echo ""
echo ""

echo "Test 6: getSequencerStatus (should be running)"
curl -s -X GET "http://localhost:2583/xrpc/com.atproto.admin.getSequencerStatus" -H "Authorization: Bearer $ACCESS_TOKEN"
echo ""
