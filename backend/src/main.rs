use std::collections::HashMap;
use std::fs;
use std::path::Path;

use dbx_plugin_sdk::{PluginHandler, PluginMetadata, PluginServer, RequestContext};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

const DEFAULT_ANTHROPIC_ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
const DEFAULT_ANTHROPIC_MODEL: &str = "claude-sonnet-4-20250514";
const DEFAULT_GEMINI_ENDPOINT: &str = "https://generativelanguage.googleapis.com";
const DEFAULT_GEMINI_MODEL: &str = "gemini-2.5-flash";
const IMPORT_METHOD: &str = "import_ai_configs";
const IMPORT_MAGPIE_METHOD: &str = "import_magpie_ai_configs";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportRequest {
    database_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MagpieImportRequest {
    config_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MagpieConfig {
    providers: Vec<MagpieProvider>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct MagpieProvider {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    key: String,
    #[serde(default)]
    key_name: String,
    #[serde(default)]
    key_protocol: String,
    #[serde(default)]
    auth_header: String,
    #[serde(default)]
    chat: String,
    #[serde(default)]
    responses: String,
    #[serde(default)]
    anthropic: String,
    #[serde(default)]
    gemini: String,
    #[serde(default)]
    models: Vec<String>,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    no_models: bool,
    #[serde(default)]
    no_api_key: bool,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    keys: Vec<MagpieKey>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct MagpieKey {
    #[serde(default)]
    name: String,
    #[serde(default)]
    key: String,
    #[serde(default)]
    protocol: String,
    #[serde(default)]
    off: bool,
}

#[derive(Debug)]
struct MagpieApiKey {
    name: String,
    value: String,
    protocol: String,
}

#[derive(Debug)]
struct MagpieImportCandidate {
    config: AiConfig,
    key_name: String,
    protocol_name: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportSkipped {
    app_type: String,
    name: String,
    reason: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportResult {
    configs: Vec<AiConfigItem>,
    skipped: Vec<ImportSkipped>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
enum AiProvider {
    Claude,
    #[serde(rename = "anthropic-compatible")]
    AnthropicCompatible,
    Gemini,
    #[serde(rename = "openai-compatible")]
    OpenaiCompatible,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "lowercase")]
enum AiApiStyle {
    #[default]
    Completions,
    Responses,
    #[serde(rename = "anthropic-messages")]
    AnthropicMessages,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
enum AiAuthMethod {
    ApiKey,
    Bearer,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
enum AiReasoningLevel {
    #[default]
    Default,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AiModelListItem {
    name: String,
    label: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    supported_effort_levels: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AiConfig {
    provider: AiProvider,
    api_key: String,
    auth_method: AiAuthMethod,
    endpoint: String,
    models: Vec<AiModelListItem>,
    model: String,
    api_style: AiApiStyle,
    custom_headers: HashMap<String, String>,
    proxy_enabled: bool,
    proxy_url: String,
    skip_tls_verify: bool,
    enable_thinking: bool,
    reasoning_level: AiReasoningLevel,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AiConfigItem {
    id: String,
    name: String,
    is_default: bool,
    #[serde(flatten)]
    config: AiConfig,
}

struct Importer;

impl PluginHandler for Importer {
    fn handle(
        &self,
        _context: RequestContext,
        method: &str,
        params: Value,
        _emitter: &dbx_plugin_sdk::PluginEmitter,
    ) -> Result<Value, dbx_plugin_sdk::PluginError> {
        if method != IMPORT_METHOD {
            if method != IMPORT_MAGPIE_METHOD {
                return Err(dbx_plugin_sdk::PluginError::method_not_found(method));
            }
            let request: MagpieImportRequest = serde_json::from_value(params)
                .map_err(|error| dbx_plugin_sdk::PluginError::new(-32602, error.to_string()))?;
            let result = import_magpie_from_path(Path::new(&request.config_path))
                .map_err(|error| dbx_plugin_sdk::PluginError::new(-32010, error))?;
            return serde_json::to_value(result)
                .map_err(|error| dbx_plugin_sdk::PluginError::new(-32603, error.to_string()));
        }
        let request: ImportRequest = serde_json::from_value(params)
            .map_err(|error| dbx_plugin_sdk::PluginError::new(-32602, error.to_string()))?;
        let result = import_from_path(Path::new(&request.database_path))
            .map_err(|error| dbx_plugin_sdk::PluginError::new(-32010, error))?;
        serde_json::to_value(result)
            .map_err(|error| dbx_plugin_sdk::PluginError::new(-32603, error.to_string()))
    }
}

fn main() -> std::io::Result<()> {
    PluginServer::new(
        PluginMetadata::new("cc-switch", "0.2.0").with_capability("ai-config-import"),
        Importer,
    )
    .serve()
}

fn import_from_path(path: &Path) -> Result<ImportResult, String> {
    if !path.is_file() {
        return Err("ccSwitchNotInstalled".to_string());
    }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("ccSwitchOpenFailed:{error}"))?;
    import_from_connection(&connection)
}

fn import_magpie_from_path(path: &Path) -> Result<ImportResult, String> {
    if !path.is_file() {
        return Err("magpieNotInstalled".to_string());
    }
    let contents = fs::read_to_string(path).map_err(|_| "magpieReadFailed".to_string())?;
    let config: MagpieConfig =
        serde_json::from_str(&contents).map_err(|_| "magpieInvalidConfig".to_string())?;
    import_magpie_config(config)
}

fn import_magpie_config(config: MagpieConfig) -> Result<ImportResult, String> {
    let mut result = ImportResult {
        configs: Vec::new(),
        skipped: Vec::new(),
    };
    for (provider_index, provider) in config.providers.into_iter().enumerate() {
        if provider.hidden {
            continue;
        }
        let provider_name = non_empty(provider.name.trim())
            .or_else(|| non_empty(provider.id.trim()))
            .unwrap_or("Magpie")
            .to_string();
        if provider.no_models {
            result.skipped.push(ImportSkipped {
                app_type: "magpie".to_string(),
                name: provider_name,
                reason: "magpieProviderHasNoModels".to_string(),
            });
            continue;
        }

        let api_keys = magpie_api_keys(&provider);
        if api_keys.is_empty() {
            result.skipped.push(ImportSkipped {
                app_type: "magpie".to_string(),
                name: provider_name,
                reason: "magpieProviderNotConfigured".to_string(),
            });
            continue;
        }

        let endpoints = magpie_endpoints(&provider);
        let models = magpie_models(&provider);
        let model = models
            .first()
            .map(|item| item.name.clone())
            .unwrap_or_default();
        let mut candidates = Vec::new();
        for (key_index, api_key) in api_keys.iter().enumerate() {
            let matching_endpoints: Vec<_> = endpoints
                .iter()
                .filter(|endpoint| {
                    (api_key.protocol.is_empty()
                        || api_key.protocol.eq_ignore_ascii_case(endpoint.protocol))
                        && !(api_key.value.is_empty() && endpoint.protocol == "gemini")
                })
                .collect();
            for endpoint in matching_endpoints {
                let mut custom_headers = provider.headers.clone();
                let auth_method = if endpoint.protocol == "anthropic" {
                    if provider.auth_header.eq_ignore_ascii_case("bearer") {
                        AiAuthMethod::Bearer
                    } else {
                        if provider.auth_header.eq_ignore_ascii_case("both") {
                            custom_headers.insert(
                                "Authorization".to_string(),
                                format!("Bearer {}", api_key.value),
                            );
                        }
                        AiAuthMethod::ApiKey
                    }
                } else {
                    AiAuthMethod::Bearer
                };
                let ai_provider = if api_key.value.is_empty() && endpoint.protocol == "anthropic" {
                    AiProvider::AnthropicCompatible
                } else {
                    endpoint.provider.clone()
                };
                let mut config = new_config(
                    ai_provider,
                    api_key.value.clone(),
                    endpoint.url.to_string(),
                    model.clone(),
                    endpoint.api_style.clone(),
                    auth_method,
                );
                config.models = models.clone();
                config.custom_headers = custom_headers;
                candidates.push(MagpieImportCandidate {
                    config,
                    key_name: if api_key.name.trim().is_empty() {
                        format!("Key {}", key_index + 1)
                    } else {
                        api_key.name.trim().to_string()
                    },
                    protocol_name: endpoint.name,
                });
            }
        }

        if candidates.is_empty() {
            result.skipped.push(ImportSkipped {
                app_type: "magpie".to_string(),
                name: provider_name,
                reason: if endpoints.is_empty() {
                    "magpieProviderNotConfigured".to_string()
                } else {
                    "magpieNoCompatibleEndpoint".to_string()
                },
            });
            continue;
        }

        let use_suffix = candidates.len() > 1;
        for (candidate_index, candidate) in candidates.into_iter().enumerate() {
            let name = if use_suffix {
                format!(
                    "{} ({} · {})",
                    provider_name, candidate.key_name, candidate.protocol_name
                )
            } else {
                provider_name.clone()
            };
            result.configs.push(AiConfigItem {
                id: format!("magpie-{provider_index}-{candidate_index}"),
                name,
                is_default: false,
                config: candidate.config,
            });
        }
    }
    Ok(result)
}

fn non_empty(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

fn magpie_api_keys(provider: &MagpieProvider) -> Vec<MagpieApiKey> {
    let mut api_keys = Vec::new();
    if !provider.key.trim().is_empty() {
        api_keys.push(MagpieApiKey {
            name: provider.key_name.trim().to_string(),
            value: provider.key.trim().to_string(),
            protocol: provider.key_protocol.trim().to_string(),
        });
    }
    for key in &provider.keys {
        if key.off || key.key.trim().is_empty() {
            continue;
        }
        api_keys.push(MagpieApiKey {
            name: key.name.trim().to_string(),
            value: key.key.trim().to_string(),
            protocol: key.protocol.trim().to_string(),
        });
    }
    if api_keys.is_empty() && provider.no_api_key {
        api_keys.push(MagpieApiKey {
            name: "No API key".to_string(),
            value: String::new(),
            protocol: provider.key_protocol.trim().to_string(),
        });
    }
    api_keys
}

struct MagpieEndpoint<'a> {
    protocol: &'static str,
    name: &'static str,
    url: &'a str,
    provider: AiProvider,
    api_style: AiApiStyle,
}

fn magpie_endpoints(provider: &MagpieProvider) -> Vec<MagpieEndpoint<'_>> {
    let mut endpoints = Vec::new();
    for (protocol, name, url, ai_provider, api_style) in [
        (
            "chat",
            "Chat Completions",
            provider.chat.as_str(),
            AiProvider::OpenaiCompatible,
            AiApiStyle::Completions,
        ),
        (
            "responses",
            "Responses",
            provider.responses.as_str(),
            AiProvider::OpenaiCompatible,
            AiApiStyle::Responses,
        ),
        (
            "anthropic",
            "Anthropic Messages",
            provider.anthropic.as_str(),
            if is_default_anthropic_endpoint(&provider.anthropic) {
                AiProvider::Claude
            } else {
                AiProvider::AnthropicCompatible
            },
            AiApiStyle::AnthropicMessages,
        ),
        (
            "gemini",
            "Gemini",
            provider.gemini.as_str(),
            AiProvider::Gemini,
            AiApiStyle::Completions,
        ),
    ] {
        if !url.trim().is_empty() {
            endpoints.push(MagpieEndpoint {
                protocol,
                name,
                url,
                provider: ai_provider,
                api_style,
            });
        }
    }
    endpoints
}

fn magpie_models(provider: &MagpieProvider) -> Vec<AiModelListItem> {
    provider
        .models
        .iter()
        .filter_map(|model| {
            let name = model.trim();
            (!name.is_empty()).then(|| AiModelListItem {
                name: name.to_string(),
                label: None,
                supported_effort_levels: Vec::new(),
            })
        })
        .collect()
}

fn import_from_connection(connection: &Connection) -> Result<ImportResult, String> {
    let providers_table_exists: bool = connection
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'providers')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("ccSwitchReadFailed:{error}"))?;
    if !providers_table_exists {
        return Err("ccSwitchInvalidDatabase".to_string());
    }

    let mut statement = connection
        .prepare(
            "SELECT app_type, name, settings_config
             FROM providers
             WHERE app_type IN ('codex', 'claude', 'gemini')
             ORDER BY is_current DESC, sort_index ASC, name COLLATE NOCASE ASC",
        )
        .map_err(|error| format!("ccSwitchReadFailed:{error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|error| format!("ccSwitchReadFailed:{error}"))?;

    let mut result = ImportResult {
        configs: Vec::new(),
        skipped: Vec::new(),
    };
    for row in rows {
        let (app_type, name, settings_config) =
            row.map_err(|error| format!("ccSwitchReadFailed:{error}"))?;
        match import_provider(&app_type, &settings_config) {
            Ok(Some(config)) => result.configs.push(AiConfigItem {
                id: format!("cc-switch-{app_type}-{}", result.configs.len()),
                name,
                is_default: false,
                config,
            }),
            Ok(None) => result.skipped.push(ImportSkipped {
                app_type,
                name,
                reason: "ccSwitchProviderNotConfigured".to_string(),
            }),
            Err(reason) => result.skipped.push(ImportSkipped {
                app_type,
                name,
                reason,
            }),
        }
    }
    Ok(result)
}

fn import_provider(app_type: &str, raw_settings: &str) -> Result<Option<AiConfig>, String> {
    let settings: Value = serde_json::from_str(raw_settings)
        .map_err(|_| "ccSwitchInvalidProviderConfig".to_string())?;
    match app_type {
        "codex" => import_codex(&settings),
        "claude" => import_claude(&settings),
        "gemini" => import_gemini(&settings),
        _ => Ok(None),
    }
}

fn import_codex(settings: &Value) -> Result<Option<AiConfig>, String> {
    let config_text = settings
        .get("config")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let document = parse_codex_toml(config_text)?;
    let model_provider = document
        .string("model_provider")
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "custom".to_string());
    let provider_section = format!("model_providers.{}", normalize_toml_key(&model_provider));
    let endpoint = document
        .string(&format!("{provider_section}.base_url"))
        .or_else(|| document.string("base_url"))
        .or_else(|| {
            find_string(
                settings,
                &["base_url", "baseUrl", "OPENAI_BASE_URL"],
                &["auth", "env", "config"],
            )
        });
    let model = document.string("model").or_else(|| {
        find_string(
            settings,
            &["model", "model_name", "modelName"],
            &["config", "env"],
        )
    });
    let api_key = find_named_string(
        settings,
        &["OPENAI_API_KEY", "api_key", "apiKey", "API_KEY"],
        &["auth", "env", "config"],
    );
    let requires_openai_auth = document
        .bool(&format!("{provider_section}.requires_openai_auth"))
        .unwrap_or(false);
    let Some(endpoint) = endpoint.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    let Some(model) = model.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    if requires_openai_auth
        && api_key
            .as_ref()
            .is_none_or(|(_, value)| value.trim().is_empty())
    {
        return Ok(None);
    }
    let api_style = document
        .string(&format!("{provider_section}.wire_api"))
        .or_else(|| document.string("wire_api"))
        .is_some_and(|value| value.eq_ignore_ascii_case("responses"))
        .then_some(AiApiStyle::Responses)
        .unwrap_or_default();
    Ok(Some(new_config(
        AiProvider::OpenaiCompatible,
        api_key.map(|(_, value)| value).unwrap_or_default(),
        endpoint,
        model,
        api_style,
        AiAuthMethod::Bearer,
    )))
}

fn import_claude(settings: &Value) -> Result<Option<AiConfig>, String> {
    let api_key = find_named_string(
        settings,
        &[
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_API_KEY",
            "api_key",
            "apiKey",
            "API_KEY",
        ],
        &["env", "auth", "config"],
    );
    let Some((key_name, api_key)) = api_key.filter(|(_, value)| !value.trim().is_empty()) else {
        return Ok(None);
    };
    let endpoint = find_string(
        settings,
        &["ANTHROPIC_BASE_URL", "base_url", "baseUrl"],
        &["env", "auth", "config"],
    )
    .unwrap_or_else(|| DEFAULT_ANTHROPIC_ENDPOINT.to_string());
    let model = find_string(
        settings,
        &["ANTHROPIC_MODEL", "model", "model_name", "modelName"],
        &["env", "config"],
    )
    .unwrap_or_else(|| DEFAULT_ANTHROPIC_MODEL.to_string());
    let provider = if is_default_anthropic_endpoint(&endpoint) {
        AiProvider::Claude
    } else {
        AiProvider::AnthropicCompatible
    };
    let auth_method = if key_name.eq_ignore_ascii_case("ANTHROPIC_AUTH_TOKEN") {
        AiAuthMethod::Bearer
    } else {
        AiAuthMethod::ApiKey
    };
    Ok(Some(new_config(
        provider,
        api_key,
        endpoint,
        model,
        AiApiStyle::AnthropicMessages,
        auth_method,
    )))
}

fn import_gemini(settings: &Value) -> Result<Option<AiConfig>, String> {
    let Some((_, api_key)) = find_named_string(
        settings,
        &[
            "GEMINI_API_KEY",
            "GOOGLE_API_KEY",
            "api_key",
            "apiKey",
            "API_KEY",
        ],
        &["env", "auth", "config"],
    )
    .filter(|(_, value)| !value.trim().is_empty()) else {
        return Ok(None);
    };
    let endpoint = find_string(
        settings,
        &[
            "GEMINI_BASE_URL",
            "GOOGLE_GEMINI_BASE_URL",
            "base_url",
            "baseUrl",
        ],
        &["env", "auth", "config"],
    )
    .unwrap_or_else(|| DEFAULT_GEMINI_ENDPOINT.to_string());
    let model = find_string(
        settings,
        &["model", "model_name", "modelName", "GEMINI_MODEL"],
        &["config", "env"],
    )
    .unwrap_or_else(|| DEFAULT_GEMINI_MODEL.to_string());
    Ok(Some(new_config(
        AiProvider::Gemini,
        api_key,
        endpoint,
        model,
        AiApiStyle::Completions,
        AiAuthMethod::ApiKey,
    )))
}

fn new_config(
    provider: AiProvider,
    api_key: String,
    endpoint: String,
    model: String,
    api_style: AiApiStyle,
    auth_method: AiAuthMethod,
) -> AiConfig {
    let model = model.trim().to_string();
    AiConfig {
        provider,
        api_key,
        auth_method,
        endpoint: endpoint.trim().trim_end_matches('/').to_string(),
        models: vec![AiModelListItem {
            name: model.clone(),
            label: None,
            supported_effort_levels: Vec::new(),
        }],
        model,
        api_style,
        custom_headers: HashMap::new(),
        proxy_enabled: false,
        proxy_url: String::new(),
        skip_tls_verify: false,
        enable_thinking: true,
        reasoning_level: AiReasoningLevel::Default,
    }
}

fn is_default_anthropic_endpoint(endpoint: &str) -> bool {
    let endpoint = endpoint.trim().trim_end_matches('/');
    endpoint.eq_ignore_ascii_case(DEFAULT_ANTHROPIC_ENDPOINT.trim_end_matches('/'))
        || endpoint.eq_ignore_ascii_case("https://api.anthropic.com")
}

fn find_string(settings: &Value, keys: &[&str], containers: &[&str]) -> Option<String> {
    find_named_string(settings, keys, containers).map(|(_, value)| value)
}

fn find_named_string(
    settings: &Value,
    keys: &[&str],
    containers: &[&str],
) -> Option<(String, String)> {
    for container in containers {
        if let Some(object) = settings.get(*container).and_then(Value::as_object) {
            if let Some(found) = object_string(object, keys) {
                return Some(found);
            }
        }
    }
    settings
        .as_object()
        .and_then(|object| object_string(object, keys))
}

fn object_string(object: &Map<String, Value>, keys: &[&str]) -> Option<(String, String)> {
    keys.iter().find_map(|key| {
        object
            .get(*key)
            .and_then(Value::as_str)
            .map(|value| ((*key).to_string(), value.to_string()))
    })
}

#[derive(Default)]
struct TomlDocument {
    values: HashMap<String, TomlScalar>,
}

#[derive(Clone)]
enum TomlScalar {
    String(String),
    Bool(bool),
}

impl TomlDocument {
    fn string(&self, key: &str) -> Option<String> {
        match self.values.get(&normalize_toml_key(key)) {
            Some(TomlScalar::String(value)) => Some(value.clone()),
            _ => None,
        }
    }

    fn bool(&self, key: &str) -> Option<bool> {
        match self.values.get(&normalize_toml_key(key)) {
            Some(TomlScalar::Bool(value)) => Some(*value),
            _ => None,
        }
    }
}

fn parse_codex_toml(input: &str) -> Result<TomlDocument, String> {
    let mut document = TomlDocument::default();
    let mut section = String::new();
    for raw_line in input.lines() {
        let line = strip_toml_comment(raw_line).trim().to_string();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            let is_array = line.starts_with("[[");
            let (opening, closing) = if is_array { ("[[", "]]") } else { ("[", "]") };
            let Some(header) = line
                .strip_prefix(opening)
                .and_then(|value| value.strip_suffix(closing))
            else {
                return Err("ccSwitchInvalidCodexConfig".to_string());
            };
            section = normalize_toml_key(header);
            continue;
        }
        let Some((raw_key, raw_value)) = line.split_once('=') else {
            continue;
        };
        let key = normalize_toml_key(raw_key);
        if key.is_empty() {
            return Err("ccSwitchInvalidCodexConfig".to_string());
        }
        let full_key = if section.is_empty() {
            key
        } else {
            format!("{section}.{key}")
        };
        if let Some(value) = parse_toml_scalar(raw_value.trim()) {
            document.values.insert(full_key, value);
        }
    }
    Ok(document)
}

fn parse_toml_scalar(value: &str) -> Option<TomlScalar> {
    if value.eq_ignore_ascii_case("true") {
        return Some(TomlScalar::Bool(true));
    }
    if value.eq_ignore_ascii_case("false") {
        return Some(TomlScalar::Bool(false));
    }
    parse_toml_string(value).map(TomlScalar::String)
}

fn parse_toml_string(value: &str) -> Option<String> {
    let value = value.trim();
    if value.starts_with('"') {
        return serde_json::from_str(value).ok();
    }
    value
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
        .map(str::to_string)
}

fn strip_toml_comment(line: &str) -> String {
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in line.char_indices() {
        if let Some(quote_character) = quote {
            if quote_character == '"' {
                if character == '\\' && !escaped {
                    escaped = true;
                    continue;
                }
                if character == '"' && !escaped {
                    quote = None;
                }
                escaped = false;
            } else if character == '\'' {
                quote = None;
            }
            continue;
        }
        match character {
            '"' | '\'' => quote = Some(character),
            '#' => return line[..index].to_string(),
            _ => {}
        }
    }
    line.to_string()
}

fn normalize_toml_key(key: &str) -> String {
    key.split('.')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(unquote_toml_key)
        .collect::<Vec<_>>()
        .join(".")
}

fn unquote_toml_key(key: &str) -> String {
    key.strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            key.strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(key)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn create_providers_database() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE providers (
                    id TEXT,
                    app_type TEXT,
                    name TEXT,
                    settings_config TEXT,
                    sort_index INTEGER,
                    is_current BOOLEAN
                );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO providers (id, app_type, name, settings_config, sort_index, is_current)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    "codex-gateway",
                    "codex",
                    "Codex Gateway",
                    serde_json::json!({
                        "auth": { "OPENAI_API_KEY": "codex-key" },
                        "config": "model_provider = \"custom\"\nmodel = \"gpt-test\"\n[model_providers.custom]\nbase_url = \"https://gateway.example\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n"
                    })
                    .to_string(),
                    0,
                    1
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO providers (id, app_type, name, settings_config, sort_index, is_current)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    "claude-gateway",
                    "claude",
                    "Claude Gateway",
                    serde_json::json!({
                        "env": {
                            "ANTHROPIC_API_KEY": "claude-key",
                            "ANTHROPIC_BASE_URL": "https://claude.example",
                            "ANTHROPIC_MODEL": "claude-test"
                        }
                    })
                    .to_string(),
                    1,
                    0
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO providers (id, app_type, name, settings_config, sort_index, is_current)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    "gemini-gateway",
                    "gemini",
                    "Gemini Gateway",
                    serde_json::json!({
                        "env": {
                            "GEMINI_API_KEY": "gemini-key",
                            "GEMINI_BASE_URL": "https://gemini.example",
                            "GEMINI_MODEL": "gemini-test"
                        }
                    })
                    .to_string(),
                    2,
                    0
                ],
            )
            .unwrap();
        connection
    }

    #[test]
    fn imports_current_codex_gateway() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE providers (id TEXT, app_type TEXT, name TEXT, settings_config TEXT, sort_index INTEGER, is_current BOOLEAN);
                 INSERT INTO providers VALUES ('gateway', 'codex', 'Gateway', '{\"auth\":{\"OPENAI_API_KEY\":\"secret\"},\"config\":\"model_provider = \\\"custom\\\"\\nmodel = \\\"gpt-test\\\"\\n[model_providers.custom]\\nbase_url = \\\"https://gateway.example\\\"\\nwire_api = \\\"responses\\\"\\nrequires_openai_auth = true\\n\"}', 0, 1);",
            )
            .unwrap();
        let result = import_from_connection(&connection).unwrap();
        assert_eq!(result.configs.len(), 1);
        assert_eq!(result.configs[0].config.model, "gpt-test");
        assert_eq!(result.configs[0].config.endpoint, "https://gateway.example");
        assert_eq!(result.configs[0].config.api_key, "secret");
        assert!(matches!(
            result.configs[0].config.api_style,
            AiApiStyle::Responses
        ));
    }

    #[test]
    fn maps_claude_auth_token_to_bearer() {
        let settings = serde_json::json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token",
                "ANTHROPIC_BASE_URL": "https://gateway.example"
            }
        });
        let config = import_claude(&settings).unwrap().unwrap();
        assert!(matches!(config.auth_method, AiAuthMethod::Bearer));
        assert!(matches!(config.provider, AiProvider::AnthropicCompatible));
    }

    #[test]
    fn maps_gemini_defaults() {
        let settings = serde_json::json!({ "env": { "GEMINI_API_KEY": "key" } });
        let config = import_gemini(&settings).unwrap().unwrap();
        assert_eq!(config.endpoint, DEFAULT_GEMINI_ENDPOINT);
        assert_eq!(config.model, DEFAULT_GEMINI_MODEL);
    }

    #[test]
    fn imports_codex_claude_and_gemini_profiles() {
        let result = import_from_connection(&create_providers_database()).unwrap();

        assert_eq!(result.configs.len(), 3);
        assert!(result.skipped.is_empty());
        assert_eq!(result.configs[0].config.api_key, "codex-key");
        assert_eq!(result.configs[0].config.model, "gpt-test");
        assert_eq!(result.configs[0].config.endpoint, "https://gateway.example");
        assert_eq!(result.configs[1].config.api_key, "claude-key");
        assert_eq!(result.configs[1].config.model, "claude-test");
        assert_eq!(result.configs[1].config.endpoint, "https://claude.example");
        assert_eq!(result.configs[2].config.api_key, "gemini-key");
        assert_eq!(result.configs[2].config.model, "gemini-test");
        assert_eq!(result.configs[2].config.endpoint, "https://gemini.example");
    }

    #[test]
    fn imports_multiple_profiles_for_the_same_app_type() {
        let connection = create_providers_database();
        connection
            .execute(
                "INSERT INTO providers (id, app_type, name, settings_config, sort_index, is_current)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    "codex-backup",
                    "codex",
                    "Codex Backup",
                    serde_json::json!({
                        "auth": { "OPENAI_API_KEY": "backup-key" },
                        "config": "model_provider = \"backup\"\nmodel = \"gpt-backup\"\n[model_providers.backup]\nbase_url = \"https://backup.example\"\nrequires_openai_auth = true\n"
                    })
                    .to_string(),
                    1,
                    0
                ],
            )
            .unwrap();

        let result = import_from_connection(&connection).unwrap();

        assert_eq!(result.configs.len(), 4);
        assert_eq!(
            result
                .configs
                .iter()
                .filter(|config| config.name.starts_with("Codex"))
                .map(|config| config.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Codex Gateway", "Codex Backup"]
        );
    }

    #[test]
    fn rejects_database_without_providers_table() {
        let connection = Connection::open_in_memory().unwrap();
        assert_eq!(
            import_from_connection(&connection).unwrap_err(),
            "ccSwitchInvalidDatabase"
        );
    }

    #[test]
    fn reports_missing_database_path() {
        let error = import_from_path(Path::new("/definitely/missing/cc-switch.db")).unwrap_err();
        assert_eq!(error, "ccSwitchNotInstalled");
    }
}
