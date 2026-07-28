//! 冻结检查——本仓库的头号缺陷是"写好了没人调"：建了表没有任何 Rust 读写、
//! 开了路由前端一次都不点、导出了函数没人引用。这类东西不会报错，只会永远不生效，
//! 于是数据库里长着一张空表、接口列表里挂着一个死链，而计划底账上却写着"已完成"。
//!
//! 这里把三类冻结物做成门禁：
//!   * **活表**：`crates/database/migrations/*.sql` 里建了且没被 drop 的表，必须在某个 crate 的 Rust 源码里出现。
//!   * **API 路由**：`echo-api` 注册的 `/api/...` 路由，必须有 `echo-web` 侧的调用方。
//!   * 两者都可以豁免，但豁免必须**显式登记**在 [`REGISTRY_PATH`]，并写清理由与去向。
//!
//! 登记表是双向的：登记了却其实已经接线的条目同样判失败，避免豁免清单变成一张
//! 只增不减的永久赦免书。

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::TaskResult;

/// 冻结豁免登记表。键是冻结物标识，值是理由（必须非空，写清"为什么允许现在没人调"）。
const REGISTRY_PATH: &str = "docs/qa/frozen-registry.json";

/// 一类冻结物的检查结论。
struct Findings {
    /// 没有任何引用、也没登记豁免的——真冻结，判失败。
    unregistered: Vec<String>,
    /// 登记了豁免、但实际已经有引用的——登记过期，判失败（否则清单会一直膨胀）。
    stale: Vec<String>,
}

pub fn check(root: &Path) -> TaskResult {
    let sources = collect_rust_sources(root)?;
    let registry = load_registry(root)?;

    let tables = check_tables(root, &sources, registry.get("tables"))?;
    let routes = check_routes(root, &sources, registry.get("routes"))?;

    let mut failures = Vec::new();
    report("活表无 Rust 读写", &tables, &mut failures);
    report("API 路由无前端调用方", &routes, &mut failures);

    if failures.is_empty() {
        eprintln!("[frozen] 通过：所有活表与 API 路由要么已接线，要么已登记豁免。");
        Ok(())
    } else {
        Err(format!(
            "冻结检查未通过（{} 项）。要么接线，要么在 {REGISTRY_PATH} 登记理由。",
            failures.len()
        ))
    }
}

fn report(label: &str, findings: &Findings, failures: &mut Vec<String>) {
    for item in &findings.unregistered {
        eprintln!("[frozen] 冻结 · {label}: {item}");
        failures.push(item.clone());
    }
    for item in &findings.stale {
        eprintln!("[frozen] 登记过期 · {label}: {item} 已有引用，请从登记表移除");
        failures.push(item.clone());
    }
}

// ───────────────────────── 输入采集 ─────────────────────────

/// 把 `crates/` 与 `xtask/` 下所有 `.rs` 拼成一个大字符串。检查只问"出现过没有"，
/// 不做语法分析——宁可漏报（有人只是在注释里提了一嘴）也不误报（挡住正常提交）。
fn collect_rust_sources(root: &Path) -> TaskResult<String> {
    let mut buf = String::new();
    for dir in ["crates", "xtask"] {
        collect_rs_into(&root.join(dir), &mut buf)?;
    }
    Ok(buf)
}

fn collect_rs_into(dir: &Path, buf: &mut String) -> TaskResult {
    let entries = std::fs::read_dir(dir)
        .map_err(|error| format!("读取目录 {} 失败: {error}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("遍历 {} 失败: {error}", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "target") {
                continue;
            }
            collect_rs_into(&path, buf)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let body = std::fs::read_to_string(&path)
                .map_err(|error| format!("读取 {} 失败: {error}", path.display()))?;
            buf.push_str(&body);
            buf.push('\n');
        }
    }
    Ok(())
}

fn load_registry(root: &Path) -> TaskResult<BTreeMap<String, BTreeMap<String, String>>> {
    let path = root.join(REGISTRY_PATH);
    let body = std::fs::read_to_string(&path)
        .map_err(|error| format!("读取 {} 失败: {error}", path.display()))?;
    let value: Value = serde_json::from_str(&body)
        .map_err(|error| format!("解析 {} 失败: {error}", path.display()))?;
    let object = value
        .as_object()
        .ok_or_else(|| format!("{REGISTRY_PATH} 顶层必须是对象"))?;

    let mut out = BTreeMap::new();
    for (section, entries) in object {
        // 顶层的 `_note` 之类说明字段直接跳过，别当成一节豁免。
        let Some(entries) = entries.as_object() else {
            continue;
        };
        let mut parsed = BTreeMap::new();
        for (key, reason) in entries {
            let reason = reason
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    format!("{REGISTRY_PATH} 的 {section}.{key} 必须写非空理由——没有理由的豁免等于没有登记")
                })?;
            parsed.insert(key.clone(), reason.to_string());
        }
        out.insert(section.clone(), parsed);
    }
    Ok(out)
}

// ───────────────────────── 活表 ─────────────────────────

/// 从迁移目录还原"当前还活着的表"：建过、且之后没有被 drop。
fn live_tables(root: &Path) -> TaskResult<BTreeSet<String>> {
    let dir = root.join("crates/database/migrations");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .map_err(|error| format!("读取 {} 失败: {error}", dir.display()))?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
        .collect();
    // 迁移按文件名有序应用，drop 必须在 create 之后才生效。
    files.sort();

    let mut live = BTreeSet::new();
    for path in files {
        let body = std::fs::read_to_string(&path)
            .map_err(|error| format!("读取 {} 失败: {error}", path.display()))?;
        for (verb, name) in sql_table_statements(&body) {
            match verb {
                TableVerb::Create => {
                    live.insert(name);
                }
                TableVerb::Drop => {
                    live.remove(&name);
                }
            }
        }
    }
    Ok(live)
}

enum TableVerb {
    Create,
    Drop,
}

/// 抽 `CREATE TABLE [IF NOT EXISTS] x` / `DROP TABLE [IF EXISTS] x`。
/// 用逐词扫描而不是正则：xtask 不该为了三个关键字多背一个 regex 依赖。
fn sql_table_statements(sql: &str) -> Vec<(TableVerb, String)> {
    let lowered = sql.to_ascii_lowercase();
    let words: Vec<&str> = lowered.split_whitespace().collect();
    let raw: Vec<&str> = sql.split_whitespace().collect();
    let mut out = Vec::new();
    for (index, window) in words.windows(2).enumerate() {
        let verb = match (window[0], window[1]) {
            ("create", "table") => TableVerb::Create,
            ("drop", "table") => TableVerb::Drop,
            _ => continue,
        };
        // 跳过可选的 IF NOT EXISTS / IF EXISTS。
        let mut cursor = index + 2;
        while words
            .get(cursor)
            .is_some_and(|word| matches!(*word, "if" | "not" | "exists"))
        {
            cursor += 1;
        }
        let Some(name) = raw.get(cursor) else {
            continue;
        };
        let name = name
            .trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
            .to_ascii_lowercase();
        if !name.is_empty() {
            out.push((verb, name));
        }
    }
    out
}

fn check_tables(
    root: &Path,
    sources: &str,
    registered: Option<&BTreeMap<String, String>>,
) -> TaskResult<Findings> {
    let empty = BTreeMap::new();
    let registered = registered.unwrap_or(&empty);
    let mut unregistered = Vec::new();
    let mut stale = Vec::new();
    for table in live_tables(root)? {
        // `_archive_<迁移号>_<原名>` 是退役迁移留下的只读存档快照，按定义就不该有 Rust 读写；
        // 它们不是"忘了接线"，而是"故意留着以防回滚"，逐张登记只会淹没真正的冻结物。
        if table.starts_with("_archive_") {
            continue;
        }
        let referenced = mentions_word(sources, &table);
        match (referenced, registered.contains_key(&table)) {
            (false, false) => unregistered.push(table),
            (true, true) => stale.push(table),
            _ => {}
        }
    }
    Ok(Findings {
        unregistered,
        stale,
    })
}

// ───────────────────────── API 路由 ─────────────────────────

fn check_routes(
    root: &Path,
    _sources: &str,
    registered: Option<&BTreeMap<String, String>>,
) -> TaskResult<Findings> {
    let empty = BTreeMap::new();
    let registered = registered.unwrap_or(&empty);

    let api = std::fs::read_to_string(root.join("crates/backend/echo-api/src/lib.rs"))
        .map_err(|error| format!("读取 echo-api/src/lib.rs 失败: {error}"))?;
    let mut web = String::new();
    collect_rs_into(&root.join("crates/frontend/echo-web/src"), &mut web)?;
    // E2E 验收也算真实调用方：它替用户点了这条路由。
    collect_rs_into(&root.join("crates/qa/echo-e2e/src"), &mut web)?;

    let mut unregistered = Vec::new();
    let mut stale = Vec::new();
    for route in api_routes(&api) {
        let called = route_is_called(&web, &route);
        match (called, registered.contains_key(&route)) {
            (false, false) => unregistered.push(route),
            (true, true) => stale.push(route),
            _ => {}
        }
    }
    Ok(Findings {
        unregistered,
        stale,
    })
}

/// 判定一条路由在前端有没有真实调用方。
///
/// 两种形态分开处理，否则会同时产生假阴和假阳：
///   * **带路径参数**（`/api/profiles/:ticker`）：前端是 `format!` 拼出来的，只能核到 `/:` 之前
///     的前缀。
///   * **不带参数**（`/api/ask`）：必须核到*完整*路径。只用 `contains` 的话，
///     前端里的 `"/api/ask/stream"` 会让 `/api/ask` 蒙混过关——那正是这个门禁要抓的东西。
fn route_is_called(web: &str, route: &str) -> bool {
    if let Some(prefix) = route.split_once("/:").map(|(head, _)| head) {
        return web.contains(&format!("{prefix}/"));
    }
    let bytes = web.as_bytes();
    web.match_indices(route).any(|(index, _)| {
        let after = index + route.len();
        // 路径到此为止才算命中：后面跟着 `/` 或别的路径字符说明那是另一条更长的路由。
        after >= bytes.len()
            || !matches!(bytes[after], b'/' | b'-' | b'_') && !bytes[after].is_ascii_alphanumeric()
    })
}

/// 抽 `.route("/api/…", …)` 里的路径字面量。只看 `/api/` 前缀——`/health` 这类
/// 探活端点本来就该由编排系统而不是前端来调。
fn api_routes(api_source: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (index, _) in api_source.match_indices(".route(") {
        let rest = &api_source[index..];
        let Some(open) = rest.find('"') else { continue };
        let Some(close) = rest[open + 1..].find('"') else {
            continue;
        };
        let path = &rest[open + 1..open + 1 + close];
        if path.starts_with("/api/") {
            out.insert(path.to_string());
        }
    }
    out
}

// ───────────────────────── 词边界匹配 ─────────────────────────

/// 全词匹配：`teams` 不能被 `team_memberships` 里的子串蒙混过关。
fn mentions_word(haystack: &str, needle: &str) -> bool {
    let bytes = haystack.as_bytes();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    haystack.match_indices(needle).any(|(index, _)| {
        let before_ok = index == 0 || !is_ident(bytes[index - 1]);
        let after = index + needle.len();
        let after_ok = after >= bytes.len() || !is_ident(bytes[after]);
        before_ok && after_ok
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_after_create_removes_table_from_live_set() {
        let created =
            sql_table_statements("CREATE TABLE IF NOT EXISTS \"cn_financials\" (id int);");
        assert!(
            matches!(created.as_slice(), [(TableVerb::Create, name)] if name == "cn_financials")
        );
        let dropped = sql_table_statements("DROP TABLE IF EXISTS \"cn_financials\";");
        assert!(matches!(dropped.as_slice(), [(TableVerb::Drop, name)] if name == "cn_financials"));
    }

    #[test]
    fn route_literals_are_extracted_and_health_probes_ignored() {
        let source = r#"
            .route("/api/watch/list", get(watch_list))
            .route("/healthz", get(health))
        "#;
        let routes = api_routes(source);
        assert!(routes.contains("/api/watch/list"));
        assert_eq!(routes.len(), 1, "只收 /api/ 前缀，探活端点不算前端契约");
    }

    #[test]
    fn longer_route_does_not_cover_its_own_prefix() {
        let web = r#" api::stream("/api/ask/stream") "#;
        assert!(
            !route_is_called(web, "/api/ask"),
            "SSE 版被调用不代表 REST 版有人调——那正是要抓的冻结"
        );
        assert!(route_is_called(web, "/api/ask/stream"));
    }

    #[test]
    fn parameterised_route_matches_its_format_prefix() {
        let web = r#" api::get(&format!("/api/profiles/{ticker}")) "#;
        assert!(route_is_called(web, "/api/profiles/:ticker"));
    }

    #[test]
    fn query_string_does_not_break_the_match() {
        let web = r#" api::get("/api/notifications?limit=20") "#;
        assert!(route_is_called(web, "/api/notifications"));
    }

    #[test]
    fn word_match_does_not_accept_substring_of_another_table() {
        assert!(!mentions_word("team_memberships", "teams"));
        assert!(mentions_word("FROM teams WHERE", "teams"));
    }
}
