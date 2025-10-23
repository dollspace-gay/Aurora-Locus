@echo off
echo ========================================
echo Aurora Locus - Admin Login Helper
echo ========================================
echo.
echo This will log you into the admin panel and save the session token.
echo.

set SERVER=http://localhost:3000
set USERNAME=admin.129.222.126.193
set PASSWORD=AuroraAdmin2024!

echo Logging in as: %USERNAME%
echo.

curl -X POST "%SERVER%/xrpc/com.atproto.server.createSession" ^
  -H "Content-Type: application/json" ^
  -d "{\"identifier\":\"%USERNAME%\",\"password\":\"%PASSWORD%\"}" ^
  -o admin_session.json

echo.
echo.
echo ========================================
echo Session created!
echo ========================================
echo.
echo The session details have been saved to: admin_session.json
echo.
echo To use in the admin panel:
echo 1. Open the browser console (F12)
echo 2. Go to the Application tab
echo 3. Find Local Storage for localhost:3000
echo 4. Copy the accessJwt value from admin_session.json
echo 5. Set it as 'adminToken' in localStorage
echo.
echo OR just open: http://localhost:3000/admin/login.html
echo And log in with the credentials above.
echo.
pause
