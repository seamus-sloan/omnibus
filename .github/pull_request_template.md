## Summary
- 

## Test plan
- [ ] 

## Version
By default a merge to `main` cuts a patch release. Pick the version update this
PR should trigger; labels override the default:
- [ ] **Minor** — add the `minor version` label (`vX.Y.Z` → `vX.(Y+1).0`).
- [ ] **Patch** — the default; add the `patch version` label or leave unlabeled
  (`vX.Y.Z` → `vX.Y.(Z+1)`).
- [ ] **No release** — add the `no release` label to skip cutting a release.

If both `minor version` and `patch version` are applied, `minor version` wins.
Unlabeled PRs that only touch docs (`*.md`, `docs/`) or CI (`.github/`) skip the
release automatically.

## Notes
- 

<!--
Use the following for callouts:

>[!NOTE]
> Blue message!

>[!WARNING]
> Yello message!

>[!IMPORTANT]
> Purple message!

>[!CAUTION]
> Red message!

>[!TIP]
> Green message!
-->
