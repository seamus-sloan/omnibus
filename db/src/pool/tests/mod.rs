//! Tests for pool init and migrations, split by sub-topic into the sibling
//! modules below: `init_db` error surfacing and the migrator's bookkeeping,
//! per-migration correctness checks against a fresh `sqlite::memory:`
//! database, the derived-key resets, and the boot-time repair passes.

mod boot;
mod boot_repair;
mod migrations;
mod norm_reset;
