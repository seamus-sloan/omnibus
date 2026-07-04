## Summary
- 

## Test plan
- [ ] 

## Version
Every merge to `main` cuts a release. Pick the version update this PR should
trigger and apply the matching label:
- [ ] **Minor** — add the `minor version` label (`vX.Y.Z` → `vX.(Y+1).0`).
- [ ] **Patch** — add the `patch version` label, or leave unlabeled to default
  to a patch bump (`vX.Y.Z` → `vX.Y.(Z+1)`).
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
