Release a patch version. Steps:
1. Read current version from src-tauri/tauri.conf.json
2. Increment patch (x.x.+1)
3. Update version in BOTH tauri.conf.json AND package.json
4. Stage all changes with git add -A
5. Commit with message "Release vX.X.X"
6. Push to main
7. Create tag vX.X.X and push tag
8. Confirm the GitHub Actions URL to monitor the build
