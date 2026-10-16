use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

use dbx_plugin_sdk::{PluginHandler, PluginMetadata, PluginServer, RequestContext};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

const DEFAULT_ANTHROPIC_ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
const DEFAULT_GEMINI_ENDPOINT: &str = "https://generativelanguage.googleapis.com";
const DEFAULT_MAGPIE_GATEWAY_PORT: u16 = 3425;
const MAGPIE_CODEX_GATEWAY_API_KEY: &str = "magpie-dbx";
const MAGPIE_GATEWAY_TIMEOUT: Duration = Duration::from_millis(900);
const MAX_MAGPIE_GATEWAY_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const IMPORT_METHOD: &str = "import_ai_configs";
const IMPORT_MAGPIE_METHOD: &str = "import_magpie_ai_configs";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportRequest {
    database_path: String,
    #[serde(default)]
    settings_path: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct CcSwitchSettings {
    #[serde(default)]
    visible_apps: Option<HashMap<String, bool>>,
}

impl CcSwitchSettings {
    fn is_app_visible(&self, app_type: &str) -> bool {
        let configured_visibility = self.visible_apps.as_ref().and_then(|visible_apps| {
            let aliases: &[&str] = match app_type {
                "claude-desktop" | "claude_desktop" | "claudeDesktop" => {
                    &["claude-desktop", "claudeDesktop", "claude_desktop"]
                }
                _ => &[app_type],
            };
            aliases
                .iter()
                .find_map(|key| visible_apps.get(*key).copied())
        });

        configured_visibility.unwrap_or_else(|| app_type != "hermes")
    }
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
struct MagpieStash {
    #[serde(default)]
    codex: MagpieCodexPreferences,
}

#[derive(Debug, Deserialize, Default)]
struct MagpieCodexPreferences {
    #[serde(default)]
    model: String,
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
    #[serde(default, rename = "baseAPI")]
    base_api: String,
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
    off: bool,
    #[serde(default)]
    no_models: bool,
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
        let result = import_from_path(
            Path::new(&request.database_path),
            request.settings_path.as_deref().map(Path::new),
        )
        .map_err(|error| dbx_plugin_sdk::PluginError::new(-32010, error))?;
        serde_json::to_value(result)
            .map_err(|error| dbx_plugin_sdk::PluginError::new(-32603, error.to_string()))
    }
}

fn main() -> std::io::Result<()> {
    PluginServer::new(
        PluginMetadata::new("cc-switch", env!("CARGO_PKG_VERSION")).with_capability("ai-config-import"),
        Importer,
    )
    .serve()
}

fn import_from_path(path: &Path, settings_path: Option<&Path>) -> Result<ImportResult, String> {
    if !path.is_file() {
        return Err("ccSwitchNotInstalled".to_string());
    }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("ccSwitchOpenFailed:{error}"))?;
    let settings = read_cc_switch_settings(settings_path)?;
    import_from_connection_with_settings(&connection, &settings)
}

fn read_cc_switch_settings(path: Option<&Path>) -> Result<CcSwitchSettings, String> {
    let Some(path) = path else {
        return Ok(CcSwitchSettings::default());
    };
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CcSwitchSettings::default());
        }
        Err(_) => return Err("ccSwitchSettingsReadFailed".to_string()),
    };
    serde_json::from_str(&contents).map_err(|_| "ccSwitchSettingsInvalid".to_string())
}

fn import_magpie_from_path(path: &Path) -> Result<ImportResult, String> {
    if !path.is_file() {
        return Err("magpieNotInstalled".to_string());
    }
    let contents = fs::read_to_string(path).map_err(|_| "magpieReadFailed".to_string())?;
    let config: MagpieConfig =
        serde_json::from_str(&contents).map_err(|_| "magpieInvalidConfig".to_string())?;
    let default_model = read_magpie_codex_default_model(path);
    let gateway_endpoint = magpie_gateway_endpoint(path);
    let gateway_port = magpie_gateway_port(path);
    let gateway_models = config
        .providers
        .iter()
        .any(is_magpie_codex_subscription)
        .then(|| read_magpie_gateway_models(gateway_port))
        .flatten();
    import_magpie_config(
        config,
        default_model.as_deref(),
        &gateway_endpoint,
        gateway_models.as_deref(),
    )
}

fn magpie_gateway_endpoint(providers_path: &Path) -> String {
    format!(
        "http://127.0.0.1:{}/v1",
        magpie_gateway_port(providers_path)
    )
}

fn magpie_gateway_port(providers_path: &Path) -> u16 {
    providers_path
        .parent()
        .and_then(|directory| fs::read_to_string(directory.join("settings.json")).ok())
        .and_then(|contents| magpie_gateway_port_from_json(&contents))
        .unwrap_or(DEFAULT_MAGPIE_GATEWAY_PORT)
}

fn magpie_gateway_port_from_json(contents: &str) -> Option<u16> {
    let settings: Value = serde_json::from_str(contents).ok()?;
    let raw_port = settings.get("port")?;
    let port = raw_port
        .as_u64()
        .or_else(|| raw_port.as_str()?.trim().parse::<u64>().ok())?;
    u16::try_from(port).ok().filter(|port| *port > 0)
}

fn read_magpie_codex_default_model(providers_path: &Path) -> Option<String> {
    let stash_path = providers_path.parent()?.join("stash.json");
    let contents = fs::read_to_string(stash_path).ok()?;
    magpie_codex_default_model_from_json(&contents)
}

fn magpie_codex_default_model_from_json(contents: &str) -> Option<String> {
    let stash: MagpieStash = serde_json::from_str(contents).ok()?;
    non_empty(stash.codex.model.trim()).map(str::to_string)
}

fn read_magpie_gateway_models(port: u16) -> Option<Vec<AiModelListItem>> {
    let response = magpie_gateway_get_json(port, "/v1/models")?;
    let models = magpie_gateway_models_from_json(&response);
    (!models.is_empty()).then_some(models)
}

fn magpie_gateway_get_json(port: u16, path: &str) -> Option<Value> {
    if !path.starts_with('/')
        || path.starts_with("//")
        || path.bytes().any(|byte| matches!(byte, b'\r' | b'\n'))
    {
        return None;
    }
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&address, MAGPIE_GATEWAY_TIMEOUT).ok()?;
    stream.set_read_timeout(Some(MAGPIE_GATEWAY_TIMEOUT)).ok()?;
    stream
        .set_write_timeout(Some(MAGPIE_GATEWAY_TIMEOUT))
        .ok()?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {MAGPIE_CODEX_GATEWAY_API_KEY}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    )
    .ok()?;

    let mut response = Vec::new();
    stream
        .take((MAX_MAGPIE_GATEWAY_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut response)
        .ok()?;
    if response.len() > MAX_MAGPIE_GATEWAY_RESPONSE_BYTES {
        return None;
    }

    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")?;
    let headers = std::str::from_utf8(&response[..header_end]).ok()?;
    let mut lines = headers.split("\r\n");
    let status = lines
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse::<u16>()
        .ok()?;
    if status != 200 {
        return None;
    }
    let is_chunked = lines.any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("transfer-encoding")
                && value.to_ascii_lowercase().contains("chunked")
        })
    });
    let body = &response[header_end + 4..];
    let body = if is_chunked {
        decode_chunked_http_body(body)?
    } else {
        body.to_vec()
    };
    serde_json::from_slice(&body).ok()
}

fn decode_chunked_http_body(body: &[u8]) -> Option<Vec<u8>> {
    let mut decoded = Vec::new();
    let mut cursor = 0;
    loop {
        let line_end = body[cursor..]
            .windows(2)
            .position(|window| window == b"\r\n")?
            + cursor;
        let size_line = std::str::from_utf8(&body[cursor..line_end]).ok()?;
        let size = usize::from_str_radix(size_line.split(';').next()?.trim(), 16).ok()?;
        cursor = line_end + 2;
        if size == 0 {
            return Some(decoded);
        }
        let chunk_end = cursor.checked_add(size)?;
        if chunk_end + 2 > body.len()
            || body.get(chunk_end..chunk_end + 2)? != b"\r\n"
            || decoded.len().saturating_add(size) > MAX_MAGPIE_GATEWAY_RESPONSE_BYTES
        {
            return None;
        }
        decoded.extend_from_slice(&body[cursor..chunk_end]);
        cursor = chunk_end + 2;
    }
}

fn magpie_gateway_models_from_json(response: &Value) -> Vec<AiModelListItem> {
    let Some(data) = response.get("data").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    let mut models: Vec<_> = data
        .iter()
        .take(1000)
        .filter_map(|item| {
            let name = item.get("id")?.as_str()?.trim();
            if name.is_empty() || !seen.insert(name.to_ascii_lowercase()) {
                return None;
            }
            let display_name = item
                .get("display_name")
                .or_else(|| item.get("displayName"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|label| !label.is_empty() && *label != name)
                .map(str::to_string);
            let source = magpie_gateway_model_source(item, name);
            let supported_effort_levels = item
                .get("supported_reasoning_levels")
                .or_else(|| item.get("supportedReasoningLevels"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|level| {
                    level
                        .as_str()
                        .or_else(|| level.get("effort").and_then(Value::as_str))
                        .and_then(normalize_magpie_reasoning_effort)
                })
                .collect();
            Some((
                AiModelListItem {
                    name: name.to_string(),
                    label: display_name.clone(),
                    supported_effort_levels,
                },
                display_name.unwrap_or_else(|| name.to_string()),
                source,
            ))
        })
        .collect();

    let mut label_counts = HashMap::new();
    for (_, label, _) in &models {
        *label_counts
            .entry(magpie_model_label_key(label))
            .or_insert(0usize) += 1;
    }

    for (model, label, source) in &mut models {
        if label_counts
            .get(&magpie_model_label_key(label))
            .copied()
            .unwrap_or_default()
            < 2
        {
            continue;
        }

        let label_key = magpie_model_label_key(label);
        let source = source.as_deref().unwrap_or(&model.name);
        let source_key = magpie_model_label_key(source);
        let suffix = if !source_key.is_empty() && label_key.contains(&source_key) {
            model.name.as_str()
        } else {
            source
        };
        model.label = Some(format!("{label} · {suffix}"));
    }

    let mut qualified_label_counts = HashMap::new();
    for (model, _, _) in &models {
        let label = model.label.as_deref().unwrap_or(&model.name);
        *qualified_label_counts
            .entry(magpie_model_label_key(label))
            .or_insert(0usize) += 1;
    }
    models
        .into_iter()
        .map(|(mut model, _, _)| {
            let label = model.label.as_deref().unwrap_or(&model.name);
            if qualified_label_counts
                .get(&magpie_model_label_key(label))
                .copied()
                .unwrap_or_default()
                > 1
            {
                model.label = Some(format!("{label} · {}", model.name));
            }
            model
        })
        .collect()
}

fn magpie_gateway_model_source(item: &Value, model_id: &str) -> Option<String> {
    // Magpie reports `owned_by: sub2api` for routing-group aliases too, so
    // prefer the model ID namespace when it identifies a distinct catalog.
    // This keeps otherwise identical model names aligned with Codex's source
    // groups (routing group, OpenAI, and Sub2API).
    let source = model_id
        .split_once('/')
        .map(|(prefix, _)| prefix)
        .filter(|prefix| {
            matches!(
                prefix.to_ascii_lowercase().as_str(),
                "codex"
                    | "openai"
                    | "sub2api"
                    | "routing-group"
                    | "routing_group"
                    | "routing group"
                    | "route-group"
                    | "route_group"
                    | "group"
            )
        })
        .or_else(|| {
            ["owned_by", "ownedBy", "provider", "source", "category"]
                .iter()
                .find_map(|key| {
                    item.get(*key)
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                })
        })?;

    Some(
        match source.to_ascii_lowercase().replace('_', "-").as_str() {
            "codex" => "OpenAI".to_string(),
            "openai" => "OpenAI".to_string(),
            "sub2api" => "Sub2API".to_string(),
            "group" | "route-group" | "routing-group" | "routing group" => {
                "routing group".to_string()
            }
            _ => source.to_string(),
        },
    )
}

fn magpie_model_label_key(label: &str) -> String {
    label
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn normalize_magpie_reasoning_effort(effort: &str) -> Option<String> {
    match effort.trim().to_ascii_lowercase().as_str() {
        "low" => Some("low".to_string()),
        "medium" => Some("medium".to_string()),
        "high" => Some("high".to_string()),
        "xhigh" => Some("xhigh".to_string()),
        "max" | "ultra" => Some("max".to_string()),
        _ => None,
    }
}

fn import_magpie_config(
    config: MagpieConfig,
    default_model: Option<&str>,
    gateway_endpoint: &str,
    gateway_models: Option<&[AiModelListItem]>,
) -> Result<ImportResult, String> {
    let mut result = ImportResult {
        configs: Vec::new(),
        skipped: Vec::new(),
    };
    for (provider_index, provider) in config.providers.into_iter().enumerate() {
        let provider_name = non_empty(provider.name.trim())
            .or_else(|| non_empty(provider.id.trim()))
            .unwrap_or("Magpie")
            .to_string();
        if provider.hidden {
            // Hidden providers do not appear in Magpie's provider picker.
            // Keep the import dialog aligned with the entries Magpie shows.
            continue;
        }
        if provider.off {
            result.skipped.push(ImportSkipped {
                app_type: "magpie".to_string(),
                name: provider_name,
                reason: "magpieProviderDisabled".to_string(),
            });
            continue;
        }
        if provider.no_models {
            result.skipped.push(ImportSkipped {
                app_type: "magpie".to_string(),
                name: provider_name,
                reason: "magpieProviderHasNoModels".to_string(),
            });
            continue;
        }

        if is_magpie_codex_subscription(&provider) {
            let models = gateway_models
                .filter(|models| !models.is_empty())
                .map(|models| models.to_vec())
                .unwrap_or_else(|| magpie_codex_models(&provider));
            if models.is_empty() {
                result.skipped.push(ImportSkipped {
                    app_type: "magpie".to_string(),
                    name: provider_name,
                    reason: "magpieProviderHasNoModels".to_string(),
                });
                continue;
            }
            let selected_model = select_magpie_codex_default_model(&models, default_model);
            let mut config = new_config(
                AiProvider::OpenaiCompatible,
                MAGPIE_CODEX_GATEWAY_API_KEY.to_string(),
                gateway_endpoint.to_string(),
                selected_model,
                AiApiStyle::Responses,
                AiAuthMethod::Bearer,
            );
            config.models = models;
            result.configs.push(AiConfigItem {
                id: format!("magpie-{provider_index}-codex"),
                name: "Codex (Magpie)".to_string(),
                is_default: false,
                config,
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
        let mut candidates = Vec::new();
        for (key_index, api_key) in api_keys.iter().enumerate() {
            let matching_endpoints: Vec<_> = endpoints
                .iter()
                .filter(|endpoint| {
                    let selected_protocol = non_empty(api_key.protocol.trim())
                        .or_else(|| non_empty(provider.base_api.trim()));
                    selected_protocol
                        .is_none_or(|protocol| protocol.eq_ignore_ascii_case(endpoint.protocol))
                        && !(api_key.value.is_empty() && endpoint.protocol == "gemini")
                })
                .collect();
            for endpoint in matching_endpoints {
                let mut custom_headers = provider.headers.clone();
                let auth_method = match endpoint.protocol {
                    "anthropic" if provider.auth_header.eq_ignore_ascii_case("bearer") => {
                        AiAuthMethod::Bearer
                    }
                    "anthropic" => {
                        if provider.auth_header.eq_ignore_ascii_case("both") {
                            custom_headers.insert(
                                "Authorization".to_string(),
                                format!("Bearer {}", api_key.value),
                            );
                        }
                        AiAuthMethod::ApiKey
                    }
                    "gemini" => AiAuthMethod::ApiKey,
                    _ => AiAuthMethod::Bearer,
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
                    magpie_default_model(&models, default_model),
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

fn is_magpie_codex_subscription(provider: &MagpieProvider) -> bool {
    provider.id.eq_ignore_ascii_case("codex")
        && provider.key.trim().is_empty()
        && provider.keys.is_empty()
        && provider.base_api.trim().is_empty()
        && provider.chat.trim().is_empty()
        && provider.responses.trim().is_empty()
        && provider.anthropic.trim().is_empty()
        && provider.gemini.trim().is_empty()
}

fn magpie_codex_models(provider: &MagpieProvider) -> Vec<AiModelListItem> {
    let mut seen = std::collections::HashSet::new();
    provider
        .models
        .iter()
        .filter_map(|model| {
            let model = model.trim();
            if model.is_empty() {
                return None;
            }
            let name = if model.starts_with("codex/") {
                model.to_string()
            } else {
                format!("codex/{model}")
            };
            if !seen.insert(name.to_ascii_lowercase()) {
                return None;
            }
            Some(AiModelListItem {
                name,
                label: None,
                supported_effort_levels: Vec::new(),
            })
        })
        .collect()
}

fn select_magpie_codex_default_model(
    models: &[AiModelListItem],
    default_model: Option<&str>,
) -> String {
    let Some(default_model) = default_model
        .map(str::trim)
        .filter(|model| !model.is_empty())
    else {
        return String::new();
    };
    let codex_model_name = if default_model.starts_with("codex/") {
        default_model.to_string()
    } else {
        format!("codex/{default_model}")
    };
    models
        .iter()
        .find(|model| {
            model.name.eq_ignore_ascii_case(default_model)
                || model.name.eq_ignore_ascii_case(&codex_model_name)
        })
        .map(|model| model.name.clone())
        .unwrap_or_default()
}

fn magpie_default_model(models: &[AiModelListItem], default_model: Option<&str>) -> String {
    let Some(default_model) = default_model
        .map(str::trim)
        .filter(|model| !model.is_empty())
    else {
        return String::new();
    };
    models
        .iter()
        .find(|model| model.name.eq_ignore_ascii_case(default_model))
        .map(|model| model.name.clone())
        .unwrap_or_default()
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
    // A provider without any saved key entries can still be a keyless local
    // endpoint. If key records exist but are all disabled or empty, preserve
    // that disabled state instead of turning it into a keyless config.
    if api_keys.is_empty() && provider.keys.is_empty() {
        api_keys.push(MagpieApiKey {
            name: String::new(),
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

#[cfg(test)]
fn import_from_connection(connection: &Connection) -> Result<ImportResult, String> {
    import_from_connection_with_settings(connection, &CcSwitchSettings::default())
}

fn import_from_connection_with_settings(
    connection: &Connection,
    settings: &CcSwitchSettings,
) -> Result<ImportResult, String> {
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
        if !settings.is_app_visible(&app_type) {
            continue;
        }
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
        _ => Err("ccSwitchUnsupportedProviderType".to_string()),
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
    .unwrap_or_default();
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
    .unwrap_or_default();
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
        models: if model.is_empty() {
            Vec::new()
        } else {
            vec![AiModelListItem {
                name: model.clone(),
                label: None,
                supported_effort_levels: Vec::new(),
            }]
        },
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
    fn leaves_missing_gemini_model_unset() {
        let settings = serde_json::json!({ "env": { "GEMINI_API_KEY": "key" } });
        let config = import_gemini(&settings).unwrap().unwrap();
        assert_eq!(config.endpoint, DEFAULT_GEMINI_ENDPOINT);
        assert!(config.model.is_empty());
        assert!(config.models.is_empty());
    }

    #[test]
    fn leaves_missing_claude_model_unset() {
        let settings = serde_json::json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token",
                "ANTHROPIC_BASE_URL": "https://gateway.example"
            }
        });
        let config = import_claude(&settings).unwrap().unwrap();
        assert!(config.model.is_empty());
        assert!(config.models.is_empty());
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
    fn reports_cc_switch_provider_types_that_dbx_cannot_import() {
        let connection = create_providers_database();
        connection
            .execute(
                "INSERT INTO providers (id, app_type, name, settings_config, sort_index, is_current)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params!["opencode-profile", "opencode", "OpenCode Profile", "{}", 3, 0],
            )
            .unwrap();

        let result = import_from_connection(&connection).unwrap();

        assert_eq!(result.configs.len(), 3);
        assert!(result.skipped.iter().any(|item| {
            item.app_type == "opencode"
                && item.name == "OpenCode Profile"
                && item.reason == "ccSwitchUnsupportedProviderType"
        }));
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
        let error =
            import_from_path(Path::new("/definitely/missing/cc-switch.db"), None).unwrap_err();
        assert_eq!(error, "ccSwitchNotInstalled");
    }

    #[test]
    fn omits_hidden_cc_switch_apps_from_import_results() {
        let connection = create_providers_database();
        connection
            .execute(
                "INSERT INTO providers (id, app_type, name, settings_config, sort_index, is_current)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params!["opencode-profile", "opencode", "OpenCode Profile", "{}", 3, 0],
            )
            .unwrap();
        let settings: CcSwitchSettings = serde_json::from_value(serde_json::json!({
            "visibleApps": {
                "codex": true,
                "claude": false,
                "gemini": true,
                "opencode": false
            }
        }))
        .unwrap();

        let result = import_from_connection_with_settings(&connection, &settings).unwrap();

        assert_eq!(result.configs.len(), 2);
        assert!(result.skipped.is_empty());
        assert_eq!(
            result
                .configs
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Codex Gateway", "Gemini Gateway"]
        );
    }

    #[test]
    fn honors_cc_switch_visibility_aliases_and_legacy_defaults() {
        let settings: CcSwitchSettings = serde_json::from_value(serde_json::json!({
            "visibleApps": {
                "claudeDesktop": false,
                "hermes": true
            }
        }))
        .unwrap();

        assert!(!settings.is_app_visible("claude-desktop"));
        assert!(settings.is_app_visible("hermes"));
        assert!(CcSwitchSettings::default().is_app_visible("codex"));
        assert!(!CcSwitchSettings::default().is_app_visible("hermes"));
    }

    #[test]
    fn imports_magpie_model_list_without_guessing_a_default_model() {
        let config: MagpieConfig = serde_json::from_value(serde_json::json!({
            "providers": [{
                "id": "magpie-provider",
                "name": "Magpie Provider",
                "key": "dummy-test-key",
                "keyProtocol": "chat",
                "chat": "https://example.invalid/v1",
                "models": ["model-z", "model-a"]
            }]
        }))
        .unwrap();

        let result = import_magpie_config(config, None, "http://127.0.0.1:3425/v1", None).unwrap();

        assert_eq!(result.configs.len(), 1);
        assert_eq!(result.configs[0].config.model, "");
        assert_eq!(
            result.configs[0]
                .config
                .models
                .iter()
                .map(|model| model.name.as_str())
                .collect::<Vec<_>>(),
            vec!["model-z", "model-a"]
        );
    }

    #[test]
    fn omits_hidden_and_reports_disabled_magpie_providers() {
        let config: MagpieConfig = serde_json::from_value(serde_json::json!({
            "providers": [
                { "id": "hidden-provider", "name": "Hidden Provider", "hidden": true },
                { "id": "disabled-provider", "name": "Disabled Provider", "off": true }
            ]
        }))
        .unwrap();

        let result = import_magpie_config(config, None, "http://127.0.0.1:3425/v1", None).unwrap();

        assert!(result.configs.is_empty());
        assert_eq!(result.skipped.len(), 1);
        assert_eq!(result.skipped[0].name, "Disabled Provider");
        assert_eq!(result.skipped[0].reason, "magpieProviderDisabled");
    }

    #[test]
    fn imports_only_magpies_selected_base_api_endpoint() {
        let config: MagpieConfig = serde_json::from_value(serde_json::json!({
            "providers": [{
                "id": "sub2api",
                "name": "Sub2API",
                "key": "dummy-test-key",
                "baseAPI": "chat",
                "chat": "https://example.invalid/v1",
                "responses": "https://example.invalid/v1",
                "anthropic": "https://example.invalid/v1",
                "models": ["gpt-6-luna"]
            }]
        }))
        .unwrap();

        let result = import_magpie_config(config, None, "http://127.0.0.1:3425/v1", None).unwrap();

        assert_eq!(result.configs.len(), 1);
        assert_eq!(
            serde_json::to_value(&result.configs[0].config.provider).unwrap(),
            "openai-compatible"
        );
        assert_eq!(
            serde_json::to_value(&result.configs[0].config.api_style).unwrap(),
            "completions"
        );
        assert_eq!(result.configs[0].config.model, "");
    }

    #[test]
    fn applies_magpies_active_codex_model_when_provider_offers_it() {
        let config: MagpieConfig = serde_json::from_value(serde_json::json!({
            "providers": [{
                "id": "sub2api",
                "name": "Sub2API",
                "key": "dummy-test-key",
                "baseAPI": "chat",
                "chat": "https://example.invalid/v1",
                "models": ["gpt-6-sol", "gpt-6-luna"]
            }]
        }))
        .unwrap();

        let result =
            import_magpie_config(config, Some("gpt-6-luna"), "http://127.0.0.1:3425/v1", None)
                .unwrap();

        assert_eq!(result.configs.len(), 1);
        assert_eq!(result.configs[0].config.model, "gpt-6-luna");
    }

    #[test]
    fn ignores_magpies_active_model_when_provider_does_not_offer_it() {
        let config: MagpieConfig = serde_json::from_value(serde_json::json!({
            "providers": [{
                "id": "sub2api",
                "name": "Sub2API",
                "key": "dummy-test-key",
                "baseAPI": "chat",
                "chat": "https://example.invalid/v1",
                "models": ["gpt-6-sol"]
            }]
        }))
        .unwrap();

        let result =
            import_magpie_config(config, Some("gpt-6-luna"), "http://127.0.0.1:3425/v1", None)
                .unwrap();

        assert_eq!(result.configs.len(), 1);
        assert_eq!(result.configs[0].config.model, "");
    }

    #[test]
    fn reads_magpies_active_codex_model_from_stash() {
        assert_eq!(
            magpie_codex_default_model_from_json(r#"{"codex":{"model":"gpt-6-luna"}}"#),
            Some("gpt-6-luna".to_string())
        );
        assert_eq!(magpie_codex_default_model_from_json("{}"), None);
    }

    #[test]
    fn reads_magpie_gateway_port_from_settings() {
        assert_eq!(
            magpie_gateway_port_from_json(r#"{"port":4321}"#),
            Some(4321)
        );
        assert_eq!(
            magpie_gateway_port_from_json(r#"{"port":"4321"}"#),
            Some(4321)
        );
        assert_eq!(magpie_gateway_port_from_json(r#"{"port":0}"#), None);
        assert_eq!(magpie_gateway_port_from_json(r#"{"port":70000}"#), None);
        assert_eq!(magpie_gateway_port_from_json("{}"), None);
    }

    #[test]
    fn imports_magpie_codex_subscription_through_local_responses_gateway() {
        let config: MagpieConfig = serde_json::from_value(serde_json::json!({
            "providers": [{
                "id": "codex",
                "models": ["gpt-6.1-sol", "gpt-6-luna", "codex/gpt-6-luna"]
            }]
        }))
        .unwrap();

        let gateway_endpoint = "http://127.0.0.1:4321/v1";
        let result =
            import_magpie_config(config, Some("gpt-6-luna"), gateway_endpoint, None).unwrap();

        assert!(result.skipped.is_empty());
        assert_eq!(result.configs.len(), 1);
        let imported = &result.configs[0];
        assert_eq!(imported.name, "Codex (Magpie)");
        assert_eq!(
            serde_json::to_value(&imported.config.provider).unwrap(),
            "openai-compatible"
        );
        assert_eq!(imported.config.api_key, "magpie-dbx");
        assert_eq!(imported.config.endpoint, gateway_endpoint);
        assert_eq!(
            serde_json::to_value(&imported.config.api_style).unwrap(),
            "responses"
        );
        assert_eq!(imported.config.model, "codex/gpt-6-luna");
        assert_eq!(
            imported
                .config
                .models
                .iter()
                .map(|model| model.name.as_str())
                .collect::<Vec<_>>(),
            vec!["codex/gpt-6.1-sol", "codex/gpt-6-luna"]
        );
        assert_eq!(imported.config.models[1].label, None);
    }

    #[test]
    fn does_not_guess_magpie_codex_subscription_default_model() {
        let config: MagpieConfig = serde_json::from_value(serde_json::json!({
            "providers": [{
                "id": "codex",
                "models": ["gpt-6-sol", "gpt-6-luna"]
            }]
        }))
        .unwrap();

        let result = import_magpie_config(
            config,
            Some("gpt-6-astra"),
            "http://127.0.0.1:3425/v1",
            None,
        )
        .unwrap();

        assert_eq!(result.configs.len(), 1);
        assert_eq!(result.configs[0].config.model, "");
    }

    #[test]
    fn reads_full_magpie_gateway_catalog_and_display_names() {
        let response = serde_json::json!({
            "object": "list",
            "data": [
                {
                    "id": "codex/gpt-6.1-sol",
                    "display_name": "GPT-6.1 Sol",
                    "supported_reasoning_levels": [
                        { "effort": "low" },
                        { "effort": "high" },
                        { "effort": "ultra" },
                        { "effort": "future" }
                    ]
                },
                {
                    "id": "routing-group/coding",
                    "display_name": "Coding Route"
                },
                {
                    "id": "sub2api/gpt-6-luna",
                    "display_name": "GPT-6 Luna · Sub2API"
                },
                {
                    "id": "codex/gpt-6.1-sol",
                    "display_name": "Duplicate"
                }
            ]
        });
        let models = magpie_gateway_models_from_json(&response);

        assert_eq!(
            models
                .iter()
                .map(|model| model.name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "codex/gpt-6.1-sol",
                "routing-group/coding",
                "sub2api/gpt-6-luna"
            ]
        );
        assert_eq!(models[0].label.as_deref(), Some("GPT-6.1 Sol"));
        assert_eq!(
            models[0].supported_effort_levels,
            vec!["low", "high", "max"]
        );
        assert_eq!(models[1].label.as_deref(), Some("Coding Route"));
    }

    #[test]
    fn uses_live_magpie_catalog_for_the_import_model_picker() {
        let config: MagpieConfig = serde_json::from_value(serde_json::json!({
            "providers": [{
                "id": "codex",
                "models": ["gpt-6-luna"]
            }]
        }))
        .unwrap();
        let gateway_models = magpie_gateway_models_from_json(&serde_json::json!({
            "data": [
                { "id": "codex/gpt-6.1-sol", "display_name": "GPT-6.1 Sol" },
                { "id": "routing-group/fast", "display_name": "Fast Route" },
                { "id": "sub2api/gpt-6-luna", "display_name": "GPT-6 Luna · Sub2API" }
            ]
        }));
        let result = import_magpie_config(
            config,
            Some("gpt-6.1-sol"),
            "http://127.0.0.1:3425/v1",
            Some(&gateway_models),
        )
        .unwrap();

        assert_eq!(result.configs.len(), 1);
        let imported = &result.configs[0].config;
        assert_eq!(imported.model, "codex/gpt-6.1-sol");
        assert_eq!(
            imported
                .models
                .iter()
                .map(|model| model.name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "codex/gpt-6.1-sol",
                "routing-group/fast",
                "sub2api/gpt-6-luna"
            ]
        );
        assert_eq!(imported.models[1].label.as_deref(), Some("Fast Route"));
    }
}
