@echo off
echo ========================================
echo Aurora Locus - Create Admin Account
echo ========================================
echo.

REM Check if sqlite3 is available
where sqlite3 >nul 2>nul
if %errorlevel% neq 0 (
    echo ERROR: sqlite3 not found!
    echo.
    echo Please install SQLite3 or use DB Browser for SQLite to run the SQL script.
    echo SQL script location: create_admin.sql
    echo Database location: data\account.sqlite
    echo.
    pause
    exit /b 1
)

echo Creating admin account in database...
echo.

sqlite3 data\account.sqlite < create_admin.sql

if %errorlevel% equ 0 (
    echo.
    echo ========================================
    echo SUCCESS! Admin account created
    echo ========================================
    echo.
    echo Login Credentials:
    echo   URL: http://localhost:3000/admin/login.html
    echo   Username: admin.129.222.126.193
    echo   Password: AuroraAdmin2024!
    echo.
) else (
    echo.
    echo ERROR: Failed to create admin account
    echo Check that the server is not running and try again
    echo.
)

pause
