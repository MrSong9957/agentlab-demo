→ [返回汇总报告](../2026-11-01-039-web-access.md)

# HTTP 与 SSRF 专家

已核实仓库依赖：reqwest 0.13.4、url 2.5.8 已在 lockfile；blocking feature 尚未开启。

**我是谁**：Rust HTTP 客户端/SSRF 防护专家。

**离谱目标**：常规"先查后连"，连接时 DNS 重解析或跳转一发生就破防（TOCTOU）；我的目标是校验过的地址＝实际连接的地址，每跳、每次解析都过闸。reqwest 自带 `dns_resolver(Resolve)` 钩子，闸装进解析层就做得到。

**我的方案**：
1. 自实现 `Resolve`：解析后逐 IP 判公网，只把放行地址交回连接，重绑定竞态消失；现成参考 reqwest-ssrf-guard（仅 async，blocking 需自写同逻辑）。
2. URL 交 url crate（已有 2.5.8）：WHATWG 解析自动归一化 `0x7f.1`→127.0.0.1，IPv6 zone id 解析直接报错即拒；`::ffff:` 先 `to_ipv4_mapped` 转 v4 再判；手写拒绝 CIDR：0.0.0.0/8、100.64/10、169.254/16、64:ff9b::/96（std `is_global` 1.87 已稳，可做基线但别全信）。
3. `redirect(Policy::none)` 手写 ≤5 跳：每跳只许 http/https、重过 IP 闸、请求重建、不透传 Cookie/Authorization（自动跳转的跨域剥头逻辑得自己做）。
4. blocking 的 `timeout/connect_timeout` 只限单次 IO：另设总墙钟 deadline，chunk 流式计满 5 MiB 即断；禁用环境代理，socks5h/HTTP_PROXY 会旁路解析闸。

**成立前提**：IP 字面量 URL 不触发解析器，请求前必须单独校验补位；连接池复用已校验连接属安全。

**验证方法**：以 OWASP SSRF Prevention Cheat Sheet 为外部标准，实测 127.0.0.1、169.254.169.254、`0x7f.1`、重绑定测试域全拒、公网站点全通。

## 来源

- [reqwest ClientBuilder（dns_resolver / resolve_to_addrs）](https://docs.rs/reqwest/latest/reqwest/struct.ClientBuilder.html)
- [reqwest::dns 模块（Resolve trait）](https://docs.rs/reqwest/latest/reqwest/dns/)
- [reqwest-ssrf-guard（含代理旁路、IP 字面量缺口说明）](https://docs.rs/reqwest-ssrf-guard)
- [reqwest redirect Policy（Policy::none、循环检测）](https://docs.rs/reqwest/latest/reqwest/redirect/struct.Policy.html)
- [reqwest blocking ClientBuilder（仅 timeout/connect_timeout）](https://docs.rs/reqwest/latest/reqwest/blocking/struct.ClientBuilder.html)
- [WHATWG IPv4 parser（十六进制/八进制归一化）](https://url.spec.whatwg.org/#concept-ipv4-parser)
- [Rust #27709（is_global 于 1.87 稳定）](https://github.com/rust-lang/rust/issues/27709)
- [HN：IPv6 zones in URLs are a mistake（rust-url 拒绝 zone id）](https://news.ycombinator.com/item?id=48405019)
