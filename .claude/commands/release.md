Release a patch version. Steps:
1. Read current version from src-tauri/tauri.conf.json
2. Increment patch (x.x.+1)
3. Update version in BOTH tauri.conf.json AND package.json
4. Stage all changes with git add -A
5. Commit with message "Release vX.X.X"
6. Push to main
7. Create tag vX.X.X and push tag
8. Confirm the GitHub Actions URL to monitor the build

## Publish Featurebase changelog entry (after tag pushed)

Surfaces the release inside the app via the "What's New" button (Featurebase popup widget, initialised in App.jsx). Runs after the tag is pushed so the entry always references a shipped version.

**Key-scope constraint (hard rule):** The FB API key in .env.local is unscoped (FB does not offer scoped keys). This /release command must only ever call `https://do.featurebase.app/v2/changelogs` and `/v2/changelogs/{id}/*` endpoints. Never use FB_API_KEY against `/v2/posts`, `/v2/users`, `/v2/comments`, or any other namespace. If a future step needs a different FB capability, request a separate key from the founder rather than reusing this one.

9. Read FB_API_KEY from .env.local (gitignored). If missing, stop and ask the founder to add it before continuing. Never echo the key to the chat.

10. Draft bullets:
   - Get commits between the previous tag and HEAD with: `git log --no-merges <prev-tag>..HEAD --pretty=format:"%s%n%b%n---"`
   - Skip the "Release vX.X.X" commit itself.
   - Group bullets into three categories: **New**, **Improved**, **Fixed**. Classify by commit-message intent (a new feature = New, an enhancement = Improved, a bugfix = Fixed). The `Macros:` / `Clipboard:` / `Website:` prefixes are area tags not category tags — read the body for intent.
   - Each bullet must be plain user-facing language. No `commit hashes`, no file paths, no internal names like "feedback_*_pattern". Audience is a non-technical Trigr user reading inside the app.
   - Apply CLAUDE.md writing rules: NO em-dashes. NO "It's not X — it's Y" framing. NO triple-parallel structures. Sentences end with full stops.
   - Skip changes that aren't user-visible (CI tweaks, internal refactors, dependency bumps with no behavioural change, website-only fixes that don't ship in the app).

11. Present the draft to the founder in chat as markdown — title (`vX.X.X`) + categorised bullets. Wait for explicit approval or edits. Iterate if asked. Do NOT POST until the founder says go.

12. On approval (or no-approval mode for backfills), POST to Featurebase. This is a **two-step flow** — `state: "live"` at creation is ignored, the entry is always created as draft until the publish endpoint is called explicitly with the locale array.

   **Step 12a — create:**

   ```powershell
   $key = (Get-Content .env.local | Where-Object { $_ -match '^FB_API_KEY=' } | ForEach-Object { ($_ -split '=', 2)[1].Trim() })
   $md = @"
### New

- ...

### Improved

- ...

### Fixed

- ...
"@
   $body = @{
       title = "vX.X.X"
       markdownContent = $md
       categories = @("New", "Improved", "Fixed")  # only include categories that have bullets
       state = "live"  # accepted by API but ignored; publish step below is required
   } | ConvertTo-Json -Depth 4

   $resp = Invoke-RestMethod -Uri "https://do.featurebase.app/v2/changelogs" `
       -Method POST `
       -Headers @{ Authorization = "Bearer $key"; "Content-Type" = "application/json" } `
       -Body $body
   $entryId = $resp.id
   ```

   **Step 12b — publish:**

   ```powershell
   Invoke-RestMethod -Uri "https://do.featurebase.app/v2/changelogs/$entryId/publish" `
       -Method POST `
       -Headers @{ Authorization = "Bearer $key"; "Content-Type" = "application/json" } `
       -Body (@{ locales = @("en") } | ConvertTo-Json)
   ```

   The publish response includes `state: "live"` and `isPublished: true`. Both must be true before reporting success to the founder. If either is missing, the entry is still draft and users won't see it.

13. Quick sanity-check: confirm via `GET https://do.featurebase.app/v2/changelogs?limit=1` that the new entry is at the top with `state: "live"`. If anything looks off, alert the founder and stop.

## Notes

- Backfilling old versions: same two-step flow, but include `date: "<ISO8601>"` in the create body so the entry sorts to the correct release date (otherwise FB stamps it with now()).
- API key rotation: if the key leaks or needs rotation, generate a new one in the FB dashboard, replace it in .env.local. No code change needed.
- Common errors:
  - `401` — key is wrong or expired
  - `400 Validation error: body.state: Invalid enum value` — state must be `"draft"` or `"live"` (not "published")
  - `400 Validation error: body: Required` on /publish — missing the `locales` field (must be `["en"]` minimum)
  - `400 Validation error: body: Unrecognized key` — the FB API uses Zod-style strict validation; remove any extra fields
