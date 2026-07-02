## Summary
- 

## Test plan
- [ ] 

## Notes
- 
- Every merge to `main` cuts a release. By default it's a patch bump
  (`vX.Y.Z` → `vX.Y.(Z+1)`); add the `minor version` label for a minor bump
  (`vX.Y.Z` → `vX.(Y+1).0`). If both labels are applied, `minor version` wins.
- To skip the release, add the `no release` label. Unlabeled PRs that only
  touch docs (`*.md`, `docs/`) or CI (`.github/`) skip automatically.

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
