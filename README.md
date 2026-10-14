# AI Config Importer

This DBX plugin imports AI provider configurations from CC-SWITCH or Magpie.
The plugin retains the `cc-switch` plugin ID and `ai-config-import` capability
for compatibility with DBX installations.

For CC-SWITCH, the DBX host supplies the local SQLite database path and the
plugin opens it read-only. For Magpie, the plugin reads `providers.json` from
`$XDG_CONFIG_HOME/magpie/providers.json`, or from
`~/.config/magpie/providers.json` when `XDG_CONFIG_HOME` is not set. The plugin
does not modify either source or send credentials over the network.

Each enabled Magpie API key is imported for each matching configured API
protocol. Disabled keys, disabled providers, and hidden providers are skipped.
OpenAI-compatible and Anthropic-compatible providers with no saved key entries
can be imported as keyless endpoints; Gemini endpoints require a key. Provider
model lists and custom headers are preserved; an empty model list stays empty.

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
