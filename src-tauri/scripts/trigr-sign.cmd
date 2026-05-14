@echo off
REM Trigr signing wrapper. Invoked by Tauri's bundler (regular signCommand)
REM and by NSIS !uninstfinalize for the uninstaller. Kept short on purpose:
REM the entire signCommand string is embedded verbatim into the generated
REM NSIS script, and any inner double-quotes break NSIS's tokenization
REM (!uninstfinalize parses on whitespace; quotes interact with the outer
REM quoting Tauri uses). So all conditional logic lives here, not inline.
REM
REM %1 is the absolute path to the artifact to sign. trusted-signing-cli
REM is expected on PATH (added by the "Install trusted-signing-cli" CI
REM step). This script is expected on PATH too (added by the "Add signing
REM script to PATH" CI step in release.yml).

setlocal
set "TARGET=%~1"

if not exist "%TARGET%" (
    echo [sign-skip] %TARGET%
    exit /b 0
)

echo [sign] %TARGET%
trusted-signing-cli -e https://weu.codesigning.azure.net -a nodescaffold-signing -c node-public-trust -d Trigr "%TARGET%"
exit /b %ERRORLEVEL%
