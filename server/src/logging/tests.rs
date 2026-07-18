use super::*;

#[test]
fn resolve_log_dir_prefers_explicit_override() {
    let dir = resolve_log_dir(Some("/var/log/omnibus".into()), Some("/data".into()));
    assert_eq!(dir, PathBuf::from("/var/log/omnibus"));
}

#[test]
fn resolve_log_dir_falls_back_to_data_dir_logs_subdir() {
    let dir = resolve_log_dir(None, Some("/srv/data".into()));
    assert_eq!(dir, PathBuf::from("/srv/data/logs"));
}

#[test]
fn resolve_log_dir_defaults_data_dir_when_unset() {
    let dir = resolve_log_dir(None, None);
    assert_eq!(dir, PathBuf::from("./data/logs"));
}
