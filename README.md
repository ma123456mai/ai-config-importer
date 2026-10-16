<p align="center">
  <img src="assets/plugin.png" width="128" alt="AI Config Importer icon" />
</p>

<p align="center"><a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a></p>

# AI Config Importer

This DBX plugin imports AI provider configurations from CC-SWITCH or Magpie.
The plugin retains the `cc-switch` plugin ID and `ai-config-import` capability
for compatibility with DBX installations.

For CC-SWITCH, the DBX host supplies the local SQLite database path and the
`settings.json` path. The plugin opens the database read-only and reads only
the app visibility settings from the settings file. Provider configurations
belonging to hidden CC-SWITCH apps are omitted; visible saved configurations
are listed across app types. Configurations DBX supports can be selected,
while unsupported visible configurations remain listed with a reason. Source
counts use the same visible configurations and unsupported entries. For
Magpie, the plugin reads `providers.json` from
`$XDG_CONFIG_HOME/magpie/providers.json`, or from
`~/.config/magpie/providers.json` when `XDG_CONFIG_HOME` is not set. The plugin
does not modify either source, copy Magpie OAuth credentials, or send source
API keys over the network.

Each enabled Magpie API key is imported for its explicit key protocol, or the
provider's selected `baseAPI` when the key has no protocol. Disabled keys are
skipped, disabled visible providers appear as unsupported entries, and hidden
providers are omitted from the import list to match Magpie's provider picker.
OpenAI-compatible and Anthropic-compatible providers with no saved key entries
can be imported as keyless endpoints; Gemini endpoints require a key. Provider
model lists and custom headers are preserved; an empty model list stays empty.
The active Codex model from the sibling `stash.json` is used as the default only
when the provider offers that exact model. The importer never guesses a default
from model list order, and CC-SWITCH profiles without an explicit model keep the
default unset.
Magpie reasoning levels are limited to DBX's supported values; Magpie's `ultra`
level is represented as DBX's `max` level.

Magpie's signed-in Codex subscription account is imported as a Responses
configuration for Magpie's local gateway. The gateway port is read from the
sibling `settings.json`, with `3425` as the default. When the local gateway is
available, the importer requests its model catalog over loopback and shows
Magpie's enabled models, routing groups, and configured providers in the
import dialog. Model IDs stay unchanged; duplicate display names include their
Magpie owner so routes and providers remain distinguishable in DBX's model
picker. If the gateway is unavailable, the importer falls back to the
Codex models saved in `providers.json`. The imported configuration uses the
`magpie-dbx` loopback client key and does not copy or read the account's OAuth
credentials.

The native candidate is built for the DBX Store targets currently supported by
the release workflow: `darwin-arm64`, `darwin-x64`, `windows-x64`, `linux-x64`,
and `linux-arm64`. Release artifacts use the `ai-config-importer-<version>-<target>`
filename while retaining the stable `cc-switch` plugin ID for existing installs.

## Development

Install the DBX plugin CLI, then run:

```bash
node scripts/package-plugin.mjs
```

This project command packages the plugin and renames the generated package and
metadata files to the `ai-config-importer-<version>-<target>` prefix. The
release workflow uses the same command automatically. The manifest ID remains
`cc-switch`, so DBX continues to recognize existing installations as the same
plugin when upgrading.

This creates an unsigned, target-specific `.dbxp` candidate and matching
`.artifact.json` in `dist/`. Native candidates must be built separately on
each supported DBX target. The official DBX Store signs reviewed candidates
before users install them.

The source repository contains no plugin packages. Release automation builds
the candidates from the tagged source and publishes them to the GitHub
Release that triggered the workflow.
