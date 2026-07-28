//! Offline agent-QA intent corpus restored from `eb3b766`.
//!
//! Loads every case from `docs/qa/fixtures/intent-routing-corpus.json`.
//! Cases without an `intent` expectation still count toward the corpus size gate
//! so silent deletions are impossible. Non-intent fields (tickers, discovery,
//! dual listing, …) are recorded as deferred until company-identity ports land.

use echo_domain::classify_research_intent;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

const EXPECTED_CASE_COUNT: usize = 275;

#[derive(Debug, Deserialize)]
struct CorpusFile {
    case_count: usize,
    cases: Vec<CorpusCase>,
}

#[derive(Debug, Deserialize)]
struct CorpusCase {
    id: String,
    scenario: String,
    question: String,
    expect: Expect,
}

#[derive(Debug, Deserialize, Default)]
struct Expect {
    #[serde(default)]
    intent: Option<String>,
    #[serde(default)]
    hk_ticker: Option<String>,
    #[serde(default)]
    us_ticker: Option<String>,
    #[serde(default)]
    discovery: Option<serde_json::Value>,
    #[serde(default)]
    multi_holding: Option<bool>,
    #[serde(default)]
    strong_company: Option<bool>,
    #[serde(default)]
    comparison: Option<bool>,
    #[serde(default)]
    dual_leg: Option<serde_json::Value>,
}

#[derive(Debug, Default)]
struct Baseline {
    total_cases: usize,
    intent_checks: usize,
    intent_failures: Vec<String>,
    deferred_fields: BTreeMap<&'static str, usize>,
}

/// 从本 crate 向上找到仓库根（以 `docs/qa/fixtures` 存在为准），再定位语料。
///
/// 不写死 `../../`：crate 在仓库里的深度会随目录重组变化，写死的层数会在搬家那天
/// 静默失效——这次按 前端/后端/agent/数据库 分层时就正好踩中。
fn corpus_path() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        let candidate = dir.join("docs/qa/fixtures/intent-routing-corpus.json");
        if candidate.exists() {
            return candidate;
        }
        assert!(dir.pop(), "未能从 crate 目录向上找到 docs/qa/fixtures");
    }
}

fn load_corpus() -> CorpusFile {
    let raw = fs::read_to_string(corpus_path()).expect("intent-routing-corpus.json must exist");
    serde_json::from_str(&raw).expect("intent-routing-corpus.json must parse")
}

fn count_deferred(expect: &Expect, baseline: &mut Baseline) {
    if expect.hk_ticker.is_some() {
        *baseline.deferred_fields.entry("hk_ticker").or_default() += 1;
    }
    if expect.us_ticker.is_some() {
        *baseline.deferred_fields.entry("us_ticker").or_default() += 1;
    }
    if expect.discovery.is_some() {
        *baseline.deferred_fields.entry("discovery").or_default() += 1;
    }
    if expect.multi_holding.is_some() {
        *baseline.deferred_fields.entry("multi_holding").or_default() += 1;
    }
    if expect.strong_company.is_some() {
        *baseline
            .deferred_fields
            .entry("strong_company")
            .or_default() += 1;
    }
    if expect.comparison.is_some() {
        *baseline.deferred_fields.entry("comparison").or_default() += 1;
    }
    if expect.dual_leg.is_some() {
        *baseline.deferred_fields.entry("dual_leg").or_default() += 1;
    }
}

#[test]
fn restored_corpus_has_exact_case_count() {
    let corpus = load_corpus();
    assert_eq!(
        corpus.case_count, EXPECTED_CASE_COUNT,
        "fixture case_count drifted"
    );
    assert_eq!(
        corpus.cases.len(),
        EXPECTED_CASE_COUNT,
        "silent corpus deletion is forbidden"
    );
    let mut ids = corpus
        .cases
        .iter()
        .map(|c| c.id.as_str())
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(
        ids.len(),
        EXPECTED_CASE_COUNT,
        "corpus ids must stay unique"
    );
}

#[test]
fn intent_routing_corpus_baseline() {
    let corpus = load_corpus();
    let mut baseline = Baseline {
        total_cases: corpus.cases.len(),
        ..Baseline::default()
    };

    for case in &corpus.cases {
        count_deferred(&case.expect, &mut baseline);
        let Some(expected) = case.expect.intent.as_deref() else {
            continue;
        };
        baseline.intent_checks += 1;
        let actual = classify_research_intent(&case.question).as_str();
        if actual != expected {
            baseline.intent_failures.push(format!(
                "{} [{}] expected intent={expected} actual={actual} q={}",
                case.id, case.scenario, case.question
            ));
        }
    }

    let report = format!(
        "total_cases={} intent_checks={} intent_failures={} deferred={:?}",
        baseline.total_cases,
        baseline.intent_checks,
        baseline.intent_failures.len(),
        baseline.deferred_fields
    );
    eprintln!("agent-qa intent baseline: {report}");

    assert_eq!(baseline.total_cases, EXPECTED_CASE_COUNT);
    assert!(
        baseline.intent_checks > 0,
        "corpus must retain intent expectations"
    );
    assert!(
        baseline.intent_failures.is_empty(),
        "intent routing regressions:\n{}",
        baseline.intent_failures.join("\n")
    );
}
