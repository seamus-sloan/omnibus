use super::*;

fn no_env(_: &str) -> Option<String> {
    None
}

fn full_env(key: &str) -> Option<String> {
    match key {
        "OMNIBUS_MCP_URL" => Some("http://env:3000/".into()),
        "OMNIBUS_MCP_USERNAME" => Some("env-user".into()),
        "OMNIBUS_MCP_PASSWORD" => Some("env-pass".into()),
        _ => None,
    }
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn load_reads_all_three_values_from_env_and_trims_trailing_slash() {
    let cfg = Config::load(args(&[]), full_env).unwrap();
    assert_eq!(cfg.base_url, "http://env:3000");
    assert_eq!(cfg.username, "env-user");
    assert_eq!(cfg.password, "env-pass");
}

#[test]
fn load_prefers_cli_flags_over_env_values() {
    let cfg = Config::load(
        args(&["--url", "http://cli:9999", "--username", "cli-user"]),
        full_env,
    )
    .unwrap();
    assert_eq!(cfg.base_url, "http://cli:9999");
    assert_eq!(cfg.username, "cli-user");
    // Unflagged value still falls back to env.
    assert_eq!(cfg.password, "env-pass");
}

#[test]
fn load_returns_missing_when_a_value_has_no_flag_and_no_env() {
    let err = Config::load(args(&["--url", "http://x", "--username", "u"]), no_env).unwrap_err();
    assert_eq!(
        err,
        ConfigError::Missing {
            what: "password",
            flag: "--password",
            env: "OMNIBUS_MCP_PASSWORD",
        }
    );
}

#[test]
fn load_treats_an_empty_env_value_as_missing() {
    let env = |key: &str| match key {
        "OMNIBUS_MCP_URL" => Some(String::new()),
        _ => full_env(key),
    };
    let err = Config::load(args(&[]), env).unwrap_err();
    assert!(matches!(
        err,
        ConfigError::Missing {
            what: "server base URL",
            ..
        }
    ));
}

#[test]
fn load_returns_unknown_arg_for_an_unrecognized_flag() {
    let err = Config::load(args(&["--nope", "x"]), full_env).unwrap_err();
    assert_eq!(err, ConfigError::UnknownArg("--nope".into()));
}

#[test]
fn load_returns_missing_value_when_a_flag_is_last() {
    let err = Config::load(args(&["--password"]), full_env).unwrap_err();
    assert_eq!(err, ConfigError::MissingValue("--password".into()));
}
