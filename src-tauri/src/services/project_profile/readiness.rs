//! Local evidence only: this probe never writes settings, runs helpers or contacts an API.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{connection, ProjectManifest, CLAUDE_PROJECT_SETTINGS};
use crate::app_config::AppType;
use crate::live::floor::CLAUDE_PROTOCOL_SELECTORS;
use crate::services::claude_account::snapshot::store::{CredentialStore, FileStore, Slot};
use crate::store::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionStatus {
    Configured,
    Missing,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionSource {
    Global,
    Project,
    Environment,
    Login,
    Routing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InheritedConnection {
    pub status: ConnectionStatus,
    pub source: Option<ConnectionSource>,
    pub provider_name: Option<String>,
    pub reason: Option<String>,
}

impl InheritedConnection {
    fn new(
        status: ConnectionStatus,
        source: Option<ConnectionSource>,
        reason: Option<&str>,
    ) -> Self {
        Self {
            status,
            source,
            provider_name: None,
            reason: reason.map(str::to_string),
        }
    }

    fn unknown(reason: &str) -> Self {
        Self::new(ConnectionStatus::Unknown, None, Some(reason))
    }
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct DeviceSettings {
    claude_config_dir: Option<PathBuf>,
    proxy_client_host: Option<String>,
}

const AUTH_ENV: &[&str] = &[
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_CUSTOM_HEADERS",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR",
    "CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR",
    "ANTHROPIC_PROFILE",
];

#[derive(Default)]
struct Effective {
    env: BTreeMap<String, (String, ConnectionSource)>,
    helper: Option<String>,
    unsupported: bool,
}

impl Effective {
    fn merge(&mut self, config: &Value, source: ConnectionSource) -> Result<(), ()> {
        let object = config.as_object().ok_or(())?;
        if let Some(env) = object.get("env") {
            for (key, value) in env.as_object().ok_or(())? {
                let canonical = AUTH_ENV
                    .iter()
                    .chain(CLAUDE_PROTOCOL_SELECTORS)
                    .find(|name| key.eq_ignore_ascii_case(name));
                if let Some(canonical) = canonical {
                    if key.as_str() != *canonical {
                        return Err(());
                    }
                    // Claude does not accept project settings as an OAuth-token source.
                    if source == ConnectionSource::Project && key == "CLAUDE_CODE_OAUTH_TOKEN" {
                        continue;
                    }
                    self.env
                        .insert(key.clone(), (value.as_str().ok_or(())?.to_string(), source));
                }
            }
        }
        if let Some(helper) = object.get("apiKeyHelper") {
            self.helper = Some(helper.as_str().ok_or(())?.to_string());
        }
        if [
            "apiKey",
            "apiBaseUrl",
            "awsAuthRefresh",
            "awsCredentialExport",
            "gcpAuthRefresh",
        ]
        .iter()
        .any(|key| {
            object
                .get(*key)
                .is_some_and(|value| !value.as_str().is_some_and(|value| value.trim().is_empty()))
        }) {
            self.unsupported = true;
        }
        Ok(())
    }

    fn value(&self, key: &str) -> &str {
        self.env
            .get(key)
            .map(|(value, _)| value.trim())
            .unwrap_or("")
    }

    fn source(&self, key: &str) -> Option<ConnectionSource> {
        self.env.get(key).map(|(_, source)| *source)
    }

    fn connection_source(&self, credential: &str) -> Option<ConnectionSource> {
        let credential = self.source(credential);
        let address = (!self.value("ANTHROPIC_BASE_URL").is_empty())
            .then(|| self.source("ANTHROPIC_BASE_URL"))
            .flatten();
        [
            ConnectionSource::Project,
            ConnectionSource::Global,
            ConnectionSource::Environment,
        ]
        .into_iter()
        .find(|source| credential == Some(*source) || address == Some(*source))
    }
}

fn read_json(path: &Path) -> Result<Option<Value>, ()> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|_| ()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(()),
    }
}

/// Exclude only unchanged fields that restoring inheritance will remove.
fn without_managed_connection(root: &Path, local: &mut Value) -> Result<(), ()> {
    let key = format!("{}\0claude", root.to_string_lossy());
    let path = crate::config::get_home_dir()
        .join(".cc-switch/project-profile-manifests")
        .join(format!(
            "{}.json",
            crate::live::engine::sha256_hex(key.as_bytes())
        ));
    let Some(value) = read_json(&path)? else {
        return Ok(());
    };
    let manifest: ProjectManifest = serde_json::from_value(value).map_err(|_| ())?;
    if manifest.version != 1
        || manifest.project_root != root.to_string_lossy()
        || manifest.app != "claude"
    {
        return Err(());
    }
    for (key, hash) in manifest.claude_connection_hashes {
        let target = if let Some(key) = key.strip_prefix("env.") {
            local
                .get_mut("env")
                .and_then(Value::as_object_mut)
                .map(|object| (object, key))
        } else {
            local.as_object_mut().map(|object| (object, key.as_str()))
        };
        let Some((object, field)) = target else {
            continue;
        };
        let Some(value) = object.get(field) else {
            continue;
        };
        let text = serde_json::to_string(value).map_err(|_| ())?;
        if crate::live::engine::sha256_hex(text.as_bytes()) != hash {
            return Err(());
        }
        object.remove(field);
    }
    Ok(())
}

fn endpoint(raw: &str) -> Result<url::Url, ()> {
    let parsed = url::Url::parse(if raw.is_empty() {
        "https://api.anthropic.com"
    } else {
        raw
    })
    .map_err(|_| ())?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(());
    }
    Ok(parsed)
}

fn configured_provider_name(
    state: &AppState,
    effective: &Effective,
    url: &url::Url,
) -> Option<String> {
    let rows = state.db.get_provider_rows(&AppType::Claude).ok()?;
    rows.readable
        .values()
        .find(|provider| {
            let Some(env) = provider.settings_config.get("env") else {
                return false;
            };
            let get = |key: &str| env.get(key).and_then(Value::as_str).unwrap_or("").trim();
            endpoint(get("ANTHROPIC_BASE_URL")).is_ok_and(|candidate| candidate == *url)
                && get("ANTHROPIC_AUTH_TOKEN") == effective.value("ANTHROPIC_AUTH_TOKEN")
                && get("ANTHROPIC_API_KEY") == effective.value("ANTHROPIC_API_KEY")
        })
        .map(|provider| provider.name.clone())
}

fn routing(state: &AppState, url: &url::Url, device: &DeviceSettings) -> InheritedConnection {
    let unknown = |reason| {
        InheritedConnection::new(
            ConnectionStatus::Unknown,
            Some(ConnectionSource::Routing),
            Some(reason),
        )
    };
    if state.proxy_service.running_now() != Some(true) {
        return unknown(if state.proxy_service.running_now() == Some(false) {
            "proxyStopped"
        } else {
            "unsupportedAuth"
        });
    }
    let Ok(Some(live)) = read_json(&crate::live::engine::DeviceStore::for_device().state_path())
    else {
        return unknown("unsupportedAuth");
    };
    let Ok(live) = serde_json::from_value::<crate::mode::state::LiveState>(live) else {
        return unknown("invalidConfig");
    };
    let Some(app) = live.apps.get("claude") else {
        return unknown("unsupportedAuth");
    };
    if app.mode != Some(crate::mode::state::Mode::Proxy) || !app.attached {
        return unknown("unsupportedAuth");
    }
    let address =
        state.db.conn.lock().ok().and_then(|connection| {
            connection.query_row(
        "SELECT listen_address, listen_port FROM proxy_config WHERE app_type = 'claude'",
        [], |row| Ok((row.get::<_, String>(0)?, row.get::<_, u16>(1)?)),
    ).ok()
        });
    let Some((host, port)) = address else {
        return unknown("invalidConfig");
    };
    let host = device.proxy_client_host.as_deref().unwrap_or(&host);
    let expected = crate::services::proxy::proxy_origin(host, port);
    if endpoint(&expected).ok().as_ref() != Some(url) {
        return unknown("unsupportedAuth");
    }
    let Some(id) = app.proxy_route.as_deref() else {
        return unknown("unsupportedAuth");
    };
    let Ok(Some(Ok(provider))) = state.db.read_provider(&AppType::Claude, id) else {
        return unknown("unsupportedAuth");
    };
    if connection::validate_provider(&provider).is_err() {
        return unknown("unsupportedAuth");
    }
    let mut result = InheritedConnection::new(
        ConnectionStatus::Configured,
        Some(ConnectionSource::Routing),
        None,
    );
    result.provider_name = Some(provider.name);
    result
}

fn login(slot: &Slot) -> InheritedConnection {
    let unknown = || {
        InheritedConnection::new(
            ConnectionStatus::Unknown,
            Some(ConnectionSource::Login),
            Some("unsupportedAuth"),
        )
    };
    let secret = match FileStore.read(slot) {
        Ok(Some(secret)) => secret,
        Ok(None) => {
            return if cfg!(target_os = "macos") {
                unknown()
            } else {
                InheritedConnection::new(ConnectionStatus::Missing, None, None)
            };
        }
        Err(_) => return InheritedConnection::unknown("invalidConfig"),
    };
    let Some(oauth) = secret.0.get("claudeAiOauth") else {
        return if secret.0.keys().any(|key| {
            crate::services::claude_account::snapshot::ACCOUNT_KEYS.contains(&key.as_str())
        }) {
            unknown()
        } else {
            InheritedConnection::new(ConnectionStatus::Missing, None, None)
        };
    };
    let text = |key: &str| oauth.get(key).and_then(Value::as_str).unwrap_or("").trim();
    let future = |key: &str| {
        oauth
            .get(key)
            .and_then(Value::as_i64)
            .map(|timestamp| timestamp > chrono::Utc::now().timestamp_millis())
    };
    if (!text("refreshToken").is_empty() && future("refreshTokenExpiresAt") != Some(false))
        || (!text("accessToken").is_empty() && future("expiresAt") == Some(true))
    {
        InheritedConnection::new(
            ConnectionStatus::Configured,
            Some(ConnectionSource::Login),
            None,
        )
    } else {
        unknown()
    }
}

pub fn inspect(state: &AppState, project_root: Option<&Path>) -> InheritedConnection {
    inspect_inner(state, project_root)
        .unwrap_or_else(|_| InheritedConnection::unknown("invalidConfig"))
}

fn inspect_inner(state: &AppState, project_root: Option<&Path>) -> Result<InheritedConnection, ()> {
    let home = crate::config::get_home_dir();
    let device: DeviceSettings = read_json(&home.join(".cc-switch/settings.json"))?
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| ())?
        .unwrap_or_default();
    let env_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    if let (Some(env), Some(configured)) = (&env_dir, &device.claude_config_dir) {
        if env != configured {
            return Err(());
        }
    }
    let config_dir = env_dir
        .clone()
        .or(device.claude_config_dir.clone())
        .unwrap_or_else(|| home.join(".claude"));
    if !config_dir.is_absolute() || crate::config::is_wsl_path(&config_dir) {
        return Ok(InheritedConnection::unknown("unsupportedAuth"));
    }
    let mut effective = Effective::default();
    for (key, value) in std::env::vars_os() {
        let Some(key) = key.to_str() else {
            continue;
        };
        if let Some(canonical) = AUTH_ENV
            .iter()
            .chain(CLAUDE_PROTOCOL_SELECTORS)
            .find(|name| key.eq_ignore_ascii_case(name))
        {
            if key != *canonical {
                return Err(());
            }
            effective.env.insert(
                key.into(),
                (
                    value.into_string().map_err(|_| ())?,
                    ConnectionSource::Environment,
                ),
            );
        }
    }
    if let Some(global) = read_json(&config_dir.join("settings.json"))? {
        effective.merge(&global, ConnectionSource::Global)?;
    }
    if let Some(root) = project_root {
        if let Some(shared) = read_json(&root.join(".claude/settings.json"))? {
            effective.merge(&shared, ConnectionSource::Project)?;
        }
        if let Some(mut local) = read_json(&root.join(CLAUDE_PROJECT_SETTINGS))? {
            without_managed_connection(root, &mut local)?;
            effective.merge(&local, ConnectionSource::Project)?;
        }
    }
    if CLAUDE_PROTOCOL_SELECTORS
        .iter()
        .any(|key| !matches!(effective.value(key), "" | "0" | "false"))
    {
        return Ok(InheritedConnection::unknown("cloudAuth"));
    }
    if effective
        .helper
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Ok(InheritedConnection::unknown("authHelper"));
    }
    if effective.unsupported
        || [
            "ANTHROPIC_CUSTOM_HEADERS",
            "ANTHROPIC_PROFILE",
            "CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR",
            "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR",
        ]
        .iter()
        .any(|key| !effective.value(key).is_empty())
    {
        return Ok(InheritedConnection::unknown("unsupportedAuth"));
    }
    let url = endpoint(effective.value("ANTHROPIC_BASE_URL"))?;
    let token = effective.value("ANTHROPIC_AUTH_TOKEN");
    let api_key = effective.value("ANTHROPIC_API_KEY");
    if !token.is_empty() && !api_key.is_empty() {
        return Ok(InheritedConnection::unknown("unsupportedAuth"));
    }
    if [token, api_key].contains(&crate::live::project::claude::PROXY_TOKEN_PLACEHOLDER) {
        return Ok(routing(state, &url, &device));
    }
    if let Some(key) = ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"]
        .into_iter()
        .find(|key| !effective.value(key).is_empty())
    {
        let value = effective.value(key);
        if value.contains("${") || value.chars().any(char::is_control) {
            return Ok(InheritedConnection::unknown("unsupportedAuth"));
        }
        let mut result = InheritedConnection::new(
            ConnectionStatus::Configured,
            effective.connection_source(key),
            None,
        );
        result.provider_name = configured_provider_name(state, &effective, &url);
        return Ok(result);
    }
    if url.host_str() != Some("api.anthropic.com") {
        return Ok(InheritedConnection::unknown("unsupportedAuth"));
    }
    if !effective.value("CLAUDE_CODE_OAUTH_TOKEN").is_empty() {
        let token = effective.value("CLAUDE_CODE_OAUTH_TOKEN");
        if token == crate::live::project::claude::PROXY_TOKEN_PLACEHOLDER
            || token.contains("${")
            || token.chars().any(char::is_control)
        {
            return Ok(InheritedConnection::unknown("unsupportedAuth"));
        }
        return Ok(InheritedConnection::new(
            ConnectionStatus::Configured,
            effective.source("CLAUDE_CODE_OAUTH_TOKEN"),
            None,
        ));
    }
    let slot = if env_dir.is_some() || device.claude_config_dir.is_some() {
        Slot::Account(config_dir)
    } else {
        Slot::Default { home }
    };
    Ok(login(&slot))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use crate::provider::Provider;
    use crate::test_support::{EnvVars, TempUserHome};
    use serde_json::json;
    use std::fs;

    fn fixture() -> (TempUserHome, EnvVars, AppState) {
        let home = TempUserHome::new();
        let mut env = EnvVars::new().remove("CLAUDE_CONFIG_DIR");
        for key in AUTH_ENV.iter().chain(CLAUDE_PROTOCOL_SELECTORS) {
            env = env.remove(key);
        }
        let state = AppState::new(std::sync::Arc::new(Database::memory().unwrap()));
        (home, env, state)
    }

    fn write(path: &Path, value: Value) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }

    fn missing_without_native_login() -> ConnectionStatus {
        if cfg!(target_os = "macos") {
            ConnectionStatus::Unknown
        } else {
            ConnectionStatus::Missing
        }
    }

    #[test]
    #[serial_test::serial]
    fn empty_live_configuration_is_not_ready_just_because_a_provider_exists() {
        let (home, _env, state) = fixture();
        state.db.save_provider(&AppType::Claude, &Provider::with_id(
            "only-in-library".into(), "Library".into(),
            json!({"env":{"ANTHROPIC_BASE_URL":"https://provider.example","ANTHROPIC_AUTH_TOKEN":"secret"}}), None,
        )).unwrap();
        let project = tempfile::tempdir().unwrap();
        let result = inspect(&state, Some(project.path()));
        assert_eq!(result.status, missing_without_native_login());
        assert_eq!(result.provider_name, None);
        assert!(!home
            .path()
            .join(".cc-switch/project-profile-manifests")
            .exists());
        assert!(!project.path().join(".claude").exists());
    }

    #[test]
    #[serial_test::serial]
    fn layered_configuration_preserves_empty_overrides_and_never_returns_secrets() {
        let (home, _env, state) = fixture();
        let global = home.path().join(".claude/settings.json");
        let project = tempfile::tempdir().unwrap();
        let local = project.path().join(CLAUDE_PROJECT_SETTINGS);
        let provider = Provider::with_id(
            "match".into(),
            "Matching provider".into(),
            json!({"env":{"ANTHROPIC_BASE_URL":"https://api.example","ANTHROPIC_AUTH_TOKEN":"private-token"}}),
            None,
        );
        state.db.save_provider(&AppType::Claude, &provider).unwrap();
        write(&global, provider.settings_config.clone());
        write(&local, json!({"model":"project-model"}));
        let result = inspect(&state, Some(project.path()));
        assert_eq!(result.status, ConnectionStatus::Configured);
        assert_eq!(result.source, Some(ConnectionSource::Global));
        assert_eq!(result.provider_name.as_deref(), Some("Matching provider"));
        let response = serde_json::to_string(&result).unwrap();
        assert!(!response.contains("private-token"));
        assert!(!response.contains("api.example"));
        write(&local, json!({"env":{"ANTHROPIC_AUTH_TOKEN":""}}));
        assert_eq!(
            inspect(&state, Some(project.path())).status,
            ConnectionStatus::Unknown
        );
        write(
            &local,
            json!({"env":{"ANTHROPIC_AUTH_TOKEN":"different-token"}}),
        );
        let result = inspect(&state, Some(project.path()));
        assert_eq!(result.status, ConnectionStatus::Configured);
        assert_eq!(result.source, Some(ConnectionSource::Project));
        assert_eq!(result.provider_name, None);
        write(
            &local,
            json!({"env":{"ANTHROPIC_BASE_URL":"https://project.example"}}),
        );
        assert_eq!(
            inspect(&state, Some(project.path())).source,
            Some(ConnectionSource::Project)
        );
    }

    #[test]
    #[serial_test::serial]
    fn process_environment_and_official_default_endpoint_are_supported() {
        let (_home, env, state) = fixture();
        let _env = env.set("ANTHROPIC_API_KEY", "environment-secret");
        let result = inspect(&state, None);
        assert_eq!(result.status, ConnectionStatus::Configured);
        assert_eq!(result.source, Some(ConnectionSource::Environment));
        assert!(!serde_json::to_string(&result)
            .unwrap()
            .contains("environment-secret"));
    }

    #[test]
    #[serial_test::serial]
    fn helpers_cloud_headers_and_malformed_settings_are_unknown_without_execution() {
        let (home, _env, state) = fixture();
        let path = home.path().join(".claude/settings.json");
        for (config, reason) in [
            (json!({"apiKeyHelper":"must-not-run"}), "authHelper"),
            (json!({"env":{"CLAUDE_CODE_USE_BEDROCK":"1"}}), "cloudAuth"),
            (
                json!({"env":{"ANTHROPIC_CUSTOM_HEADERS":"Authorization: private"}}),
                "unsupportedAuth",
            ),
            (json!({"env":{"ANTHROPIC_API_KEY":7}}), "invalidConfig"),
            (
                json!({"env":{"anthropic_api_key":"case-variant"}}),
                "invalidConfig",
            ),
        ] {
            write(&path, config);
            let before = fs::read(&path).unwrap();
            let result = inspect(&state, None);
            assert_eq!(result.status, ConnectionStatus::Unknown);
            assert_eq!(result.reason.as_deref(), Some(reason));
            assert_eq!(fs::read(&path).unwrap(), before);
        }
        fs::write(&path, "{broken").unwrap();
        assert_eq!(
            inspect(&state, None).reason.as_deref(),
            Some("invalidConfig")
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "{broken");
    }

    #[test]
    #[serial_test::serial]
    fn unchanged_managed_connection_is_excluded_before_inheritance_is_inspected() {
        let (home, _env, state) = fixture();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        let local_path = root.join(CLAUDE_PROJECT_SETTINGS);
        let local = json!({"env":{"ANTHROPIC_BASE_URL":"https://removed.example","ANTHROPIC_AUTH_TOKEN":"removed-secret"}});
        write(&local_path, local.clone());
        let mut manifest = super::super::load_manifest(&root, "claude").unwrap();
        for key in ["ANTHROPIC_BASE_URL", "ANTHROPIC_AUTH_TOKEN"] {
            manifest.claude_connection_hashes.insert(
                format!("env.{key}"),
                crate::live::engine::sha256_hex(
                    serde_json::to_string(&local["env"][key])
                        .unwrap()
                        .as_bytes(),
                ),
            );
        }
        super::super::save_manifest(&root, "claude", &manifest).unwrap();
        let original = fs::read(&local_path).unwrap();
        assert_eq!(
            inspect(&state, Some(&root)).status,
            missing_without_native_login()
        );
        write(
            &home.path().join(".claude/settings.json"),
            json!({"env":{"ANTHROPIC_API_KEY":"inherited-secret"}}),
        );
        let result = inspect(&state, Some(&root));
        assert_eq!(result.status, ConnectionStatus::Configured);
        assert_eq!(result.source, Some(ConnectionSource::Global));
        assert_eq!(fs::read(&local_path).unwrap(), original);
        write(
            &local_path,
            json!({"env":{"ANTHROPIC_BASE_URL":"https://edited.example","ANTHROPIC_AUTH_TOKEN":"removed-secret"}}),
        );
        assert_eq!(
            inspect(&state, Some(&root)).status,
            ConnectionStatus::Unknown
        );
    }

    #[test]
    #[serial_test::serial]
    fn native_login_requires_readable_usable_credentials_and_does_not_authenticate_a_custom_endpoint(
    ) {
        let (home, _env, state) = fixture();
        let credentials = home.path().join(".claude/.credentials.json");
        write(
            &credentials,
            json!({"claudeAiOauth":{"refreshToken":"private-refresh"}}),
        );
        let result = inspect(&state, None);
        assert_eq!(result.status, ConnectionStatus::Configured);
        assert_eq!(result.source, Some(ConnectionSource::Login));
        write(
            &home.path().join(".claude/settings.json"),
            json!({"env":{"ANTHROPIC_BASE_URL":"https://custom.example"}}),
        );
        assert_eq!(inspect(&state, None).status, ConnectionStatus::Unknown);
        write(&home.path().join(".claude/settings.json"), json!({}));
        write(
            &credentials,
            json!({"claudeAiOauth":{"refreshToken":"private-refresh","refreshTokenExpiresAt":1}}),
        );
        assert_eq!(inspect(&state, None).status, ConnectionStatus::Unknown);
        fs::write(&credentials, "{broken").unwrap();
        assert_eq!(
            inspect(&state, None).reason.as_deref(),
            Some("invalidConfig")
        );
    }

    #[test]
    #[serial_test::serial]
    fn proxy_placeholder_is_not_a_static_credential_when_proxy_is_stopped() {
        let (home, _env, state) = fixture();
        write(
            &home.path().join(".claude/settings.json"),
            json!({"env":{
                "ANTHROPIC_BASE_URL":"http://127.0.0.1:15721",
                "ANTHROPIC_AUTH_TOKEN":crate::live::project::claude::PROXY_TOKEN_PLACEHOLDER
            }}),
        );
        let result = inspect(&state, None);
        assert_eq!(result.status, ConnectionStatus::Unknown);
        assert_eq!(result.source, Some(ConnectionSource::Routing));
        assert_eq!(result.reason.as_deref(), Some("proxyStopped"));
        write(
            &home.path().join(".claude/settings.json"),
            json!({"env":{
                "ANTHROPIC_BASE_URL":"http://127.0.0.1:15721",
                "ANTHROPIC_AUTH_TOKEN":"other-test-token",
                "ANTHROPIC_API_KEY":crate::live::project::claude::PROXY_TOKEN_PLACEHOLDER
            }}),
        );
        let result = inspect(&state, None);
        assert_eq!(result.status, ConnectionStatus::Unknown);
        assert_eq!(result.source, None);
        assert_eq!(result.reason.as_deref(), Some("unsupportedAuth"));
    }
}
