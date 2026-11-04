# API 金额的价格表联网获取，内置快照兜底

API 金额参照 ccusage 的算法，单价来自 LiteLLM 的 `model_prices_and_context_window.json`。新模型发布、价格调整都比应用更新频繁，只用编进应用的快照会让新模型长期"无价格"。因此应用在后台下载这份价格表（缓存超过一天才重新下载），只保留 `claude-*` 条目存到 Caches 下的 `pricing.json`；下载失败时沿用缓存，没有缓存时用编进应用的快照（`crates/codeck-core/src/pricing-snapshot.json`，由 `examples/update_pricing.rs` 刷新）。这是应用唯一的出网请求：只下载一个公开的 JSON 文件，不发送任何本机数据。

## Consequences

- 离线也能显示金额，只是新模型可能无价格，直到联网或更新快照。
- 价格表变化时重新计算全部消息的金额（实测约 1,500 个 Session、20 万条消息不到 1 秒）。
- 价格表格式由 LiteLLM 决定；无法解析时视同下载失败。
