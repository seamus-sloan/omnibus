#!/usr/bin/env bash
# Validate a decoded provisioning profile against what this build expects.
#
# Called once per profile by action.yml, before the archive. Export is
# otherwise the first step that compares a profile to a binary, and by then the
# run has spent ten minutes building — so every check here is one that would
# have cost an archive to discover.
#
# Usage: check-profile.sh <decoded-plist> <profile-name> [bundle-id] [app-group]
# An empty bundle-id or app-group skips that check.
set -euo pipefail

plist="${1:?usage: check-profile.sh <plist> <name> [bundle-id] [app-group]}"
name="${2:?}"
expected_bundle_id="${3-}"
require_app_group="${4-}"

pb() { /usr/libexec/PlistBuddy -c "$1" "$plist" 2>/dev/null; }

if [ -n "$expected_bundle_id" ]; then
  app_id="$(pb 'Print :Entitlements:application-identifier' || true)"
  if [ -z "$app_id" ]; then
    echo "::error::Profile '$name' carries no application-identifier entitlement, so it cannot sign anything."
    exit 1
  fi
  # "D33VSDXHA6.com.omnibus.mobile" -> "com.omnibus.mobile". The team prefix is
  # the first dot-separated component; the bundle id is everything after it.
  profile_bundle_id="${app_id#*.}"
  case "$profile_bundle_id" in
    *'*')
      echo "Profile '$name' is a wildcard ($profile_bundle_id) — matches $expected_bundle_id by construction."
      ;;
    "$expected_bundle_id")
      echo "Profile '$name' is for $profile_bundle_id."
      ;;
    *)
      echo "::error::Profile '$name' is for app ID '$profile_bundle_id', but this build signs '$expected_bundle_id'. Either register an App ID for '$expected_bundle_id' and generate an App Store profile for it, or change that target's PRODUCT_BUNDLE_IDENTIFIER to match the profile."
      exit 1
      ;;
  esac
fi

if [ -n "$require_app_group" ]; then
  # PlistBuddy prints an array one entry per line, inside Array { ... }.
  # Matched whole-line and literally: a substring test would accept
  # `group.com.example.other` for a required `group.com.example`, which is
  # the one way this check could pass while the entitlement is wrong.
  groups="$(pb 'Print :Entitlements:com.apple.security.application-groups' || true)"
  if printf '%s\n' "$groups" \
    | sed 's/^[[:space:]]*//; s/[[:space:]]*$//' \
    | grep -qxF -- "$require_app_group"; then
    echo "Profile '$name' carries the App Group $require_app_group."
  else
    echo "::error::Profile '$name' does not carry the App Group '$require_app_group'. Add the App Groups capability to its App ID and then REGENERATE the profile — a profile does not pick up a capability added after it was created, which is why reusing an existing one fails here."
    exit 1
  fi
fi

# An App Store profile must not carry get-task-allow. A Development profile
# exported by mistake signs and uploads without complaint, then App Store
# Connect rejects the build by email (ITMS-90163) long after the run went
# green — so it is worth two seconds here.
task_allow="$(pb 'Print :Entitlements:get-task-allow' || true)"
if [ "$task_allow" = "true" ]; then
  echo "::error::Profile '$name' is a Development profile (get-task-allow is true). App Store distribution needs an App Store profile; this one uploads fine and is rejected afterwards."
  exit 1
fi
