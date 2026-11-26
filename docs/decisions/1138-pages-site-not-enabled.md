# 1138: Leave pages.yml unchanged when the Pages site is not enabled

## Context

`pages/deploy` is red on `develop`: `actions/configure-pages` reports that the Pages site is Not Found. Decision 0221 and s226 expect the owner to set Settings > Pages > Source to GitHub Actions.

## Decision

Do not change `.github/workflows/pages.yml`. The owner enables Pages in the repository settings.

- The failure is a repository setting outside the code.
- The workflow is a protected path (`.github/workflows/**`), so an edit needs the owner's approval.
- Adding an `enablement` input without knowing that the token is allowed to use it would be a guess.

## Alternative

Set `enablement: true` on `configure-pages`, so the workflow creates the site itself. Not chosen: whether the workflow's `GITHUB_TOKEN` can enable Pages was not verified, and the action may need a different token for it. It would be the right choice if that is confirmed (or the owner supplies a suitable token) and the owner wants the site to be recreated automatically whenever it is missing.

## Consequence

`pages/deploy` stays red until the owner makes the setting and the workflow runs again.
