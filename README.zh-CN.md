<p align="center">
  <img src="assets/plugin.png" width="128" alt="AI 配置导入插件图标" />
</p>

<p align="center"><a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a></p>

# AI 配置导入

这是一个 DBX 插件，可从 CC-SWITCH 或 Magpie 导入 AI 提供商配置。
为兼容已有 DBX 安装，插件继续使用 `cc-switch` 插件 ID 和
`ai-config-import` 能力标识。

## 配置读取与导入

### CC-SWITCH

DBX 主程序会向插件提供本机 SQLite 数据库路径和 `settings.json` 路径。
插件以只读方式打开数据库，并且只从设置文件读取应用可见性配置。被
CC-SWITCH 隐藏的应用类型不会出现在导入列表中；可见应用类型中的已保存
配置会一起列出。DBX 支持的配置可以勾选导入，不支持的可见配置仍会列出，
并显示不支持原因。来源计数也按照这些可见配置及不支持项统计。

### Magpie

插件从以下位置读取 `providers.json`：

- 设置了 `XDG_CONFIG_HOME` 时：`$XDG_CONFIG_HOME/magpie/providers.json`
- 未设置 `XDG_CONFIG_HOME` 时：`~/.config/magpie/providers.json`

插件不会修改上述来源文件，不会复制 Magpie OAuth 凭据，也不会通过网络发送
来源配置中的 API Key。

每个已启用的 Magpie API Key 都会按其明确指定的协议导入；如果 Key 未指定
协议，则使用该提供商所选的 `baseAPI`。已禁用的 Key 会跳过；已禁用但可见的
提供商会作为不支持项列出；隐藏的提供商会省略，以保持与 Magpie 提供商选择器
一致。没有已保存 Key 的 OpenAI 兼容和 Anthropic 兼容提供商可以作为无 Key
端点导入；Gemini 端点必须配置 Key。提供商的模型列表和自定义请求头会保留，
空模型列表也会继续保持为空。

只有当 Magpie 当前激活的 Codex 模型确实存在于对应提供商中时，插件才会将它
设为默认模型。插件不会根据模型列表顺序猜测默认模型；CC-SWITCH 配置未明确
指定模型时，默认模型保持未设置。Magpie 的推理强度只映射到 DBX 支持的值；
Magpie 的 `ultra` 会映射为 DBX 的 `max`。

## Magpie Codex 订阅与模型目录

Magpie 中已登录的 Codex 订阅账号会作为一个指向 Magpie 本机网关的 Responses
配置导入。网关端口从同级的 `settings.json` 读取，默认端口为 `3425`。

本机网关可访问时，插件会通过回环地址请求模型目录，并在导入弹框中列出
Magpie 当前启用的模型、路由组和已配置提供商。模型 ID 保持不变；如果模型
显示名称重复，会附加其在 Magpie 中所属的路由或提供商名称，方便在 DBX 模型
选择器中区分。网关不可用时，插件会回退到 `providers.json` 中保存的 Codex
模型列表。

导入配置使用 `magpie-dbx` 本机回环客户端 Key，不会读取或复制账号的 OAuth
凭据。

## 构建目标与包名

当前发布工作流支持以下 DBX Store 构建目标：

- `darwin-arm64`
- `darwin-x64`
- `windows-x64`
- `linux-x64`
- `linux-arm64`

发布产物使用 `ai-config-importer-<版本号>-<目标平台>` 文件名，同时保留稳定的
`cc-switch` 插件 ID，确保 DBX 能将其识别为现有插件的升级版本。

## 开发与本地打包

先安装 DBX 插件 CLI，然后在项目目录中运行：

```bash
node scripts/package-plugin.mjs
```

该项目命令会打包插件，并将生成的安装包和元数据文件统一命名为
`ai-config-importer-<版本号>-<目标平台>` 前缀。发布工作流也会自动运行同一命令。
插件清单中的 ID 仍为 `cc-switch`，所以 DBX 升级时会继续将其识别为同一个插件。

命令会在 `dist/` 目录中生成一个未签名、针对当前平台构建的 `.dbxp` 候选包，
以及对应的 `.artifact.json` 元数据。原生候选包需要分别在每个受支持的 DBX
目标平台上构建。用户安装前，DBX 官方商店会对审核通过的候选包进行签名。

源码仓库不包含插件安装包。发布自动化会从带标签的源码构建候选包，并将产物
上传到触发工作流的 GitHub Release。

## 相关链接

- [英文说明](README.md)
- [DBX 插件开发文档](https://dbxio.com/cn/docs/plugin-development)
- [源码仓库](https://github.com/ma123456mai/ai-config-importer)
