//! Runtime configuration: instance base URL plus the login credentials.
//! Sourced from CLI flags (which win) or `OMNIBUS_MCP_*` env vars; see the
//! crate docs for the table. Parsing takes the env as a lookup closure so
//! tests never touch process-global state.

/// Base URL + credentials for one Omnibus instance.
#[derive(Clone)]
pub struct Config {
    /// Instance origin, trailing slashes trimmed (`http://host:3000`).
    pub base_url: String,
    pub username: String,
    pub password: String,
}

/// Hand-written so the plaintext password can never reach a log via a stray
/// `tracing::debug!(?config)` (same rationale as
/// `omnibus_shared::LoginRequest`, which skips `Debug` entirely).
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("base_url", &self.base_url)
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

/// Why the configuration could not be assembled.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("missing {what}: pass {flag} or set {env}")]
    Missing {
        what: &'static str,
        flag: &'static str,
        env: &'static str,
    },
    #[error("unknown argument {0}")]
    UnknownArg(String),
    #[error("flag {0} expects a value")]
    MissingValue(String),
}

const URL_ENV: &str = "OMNIBUS_MCP_URL";
const USERNAME_ENV: &str = "OMNIBUS_MCP_USERNAME";
const PASSWORD_ENV: &str = "OMNIBUS_MCP_PASSWORD";

impl Config {
    /// Assemble the config from CLI args (binary name already stripped) and
    /// an env lookup. A flag overrides its env var; every value is required.
    pub fn load<I>(args: I, env: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError>
    where
        I: IntoIterator<Item = String>,
    {
        let mut url = None;
        let mut username = None;
        let mut password = None;

        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let slot = match arg.as_str() {
                "--url" => &mut url,
                "--username" => &mut username,
                "--password" => &mut password,
                _ => return Err(ConfigError::UnknownArg(arg)),
            };
            match args.next() {
                Some(value) => *slot = Some(value),
                None => return Err(ConfigError::MissingValue(arg)),
            }
        }

        let require = |value: Option<String>,
                       env_key: &'static str,
                       what: &'static str,
                       flag: &'static str| {
            value
                .or_else(|| env(env_key).filter(|v| !v.is_empty()))
                .ok_or(ConfigError::Missing {
                    what,
                    flag,
                    env: env_key,
                })
        };

        let base_url = require(url, URL_ENV, "server base URL", "--url")?;
        Ok(Config {
            base_url: base_url.trim_end_matches('/').to_string(),
            username: require(username, USERNAME_ENV, "username", "--username")?,
            password: require(password, PASSWORD_ENV, "password", "--password")?,
        })
    }
}

#[cfg(test)]
mod tests;
