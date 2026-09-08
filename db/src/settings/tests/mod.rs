//! Unit tests for the `settings` module, split by sub-topic into the
//! sibling modules below. Env-var seeding is serialized through the
//! process-global `EnvVarGuard`.

mod libraries;
mod metadata_precedence;
mod provider_keys;
mod roundtrip;
mod smtp;
