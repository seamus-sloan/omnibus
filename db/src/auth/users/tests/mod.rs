//! Tests for user account management, split by sub-topic into the sibling
//! modules below; the raw user seeding fixture they share lives here.

mod admin;
mod display_name;
mod passwords;
mod preferences;
mod registration;

use omnibus_shared::UserPermissions;

const READER: UserPermissions = UserPermissions {
    is_admin: false,
    can_upload: false,
    can_edit: false,
    can_download: true,
};

const ADMIN: UserPermissions = UserPermissions {
    is_admin: true,
    can_upload: true,
    can_edit: true,
    can_download: true,
};
