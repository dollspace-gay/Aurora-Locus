@echo off
cd /d "%~dp0"
echo Granting admin role...
echo.

powershell -NoProfile -ExecutionPolicy Bypass -Command "$db = 'data/account.sqlite'; Add-Type -TypeDefinition @' using System; using System.Runtime.InteropServices; public class SQLite { [DllImport(\"winsqlite3.dll\", EntryPoint=\"sqlite3_open\")] public static extern int Open(string filename, out IntPtr db); [DllImport(\"winsqlite3.dll\", EntryPoint=\"sqlite3_exec\")] public static extern int Exec(IntPtr db, string sql, IntPtr callback, IntPtr arg, out IntPtr errMsg); [DllImport(\"winsqlite3.dll\", EntryPoint=\"sqlite3_close\")] public static extern int Close(IntPtr db); } '@; $dbPtr = [IntPtr]::Zero; $result = [SQLite]::Open($db, [ref]$dbPtr); if ($result -eq 0) { $sql = \"INSERT OR REPLACE INTO admin_role (did, role, granted_by, granted_at, revoked, notes) VALUES ('did:web:admin.129.222.126.193.0.0.0.0', 'admin', 'system', datetime('now'), 0, 'Manual admin setup');\"; $errMsg = [IntPtr]::Zero; $execResult = [SQLite]::Exec($dbPtr, $sql, [IntPtr]::Zero, [IntPtr]::Zero, [ref]$errMsg); if ($execResult -eq 0) { Write-Host 'SUCCESS: Admin role granted!'; } else { Write-Host 'ERROR: Failed to grant admin role'; }; [SQLite]::Close($dbPtr); } else { Write-Host 'ERROR: Failed to open database'; }"

echo.
echo Done!
pause
