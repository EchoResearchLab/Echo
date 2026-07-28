//! 公司别名、港美双重上市与 ADR 映射的唯一底账。
//!
//! 从 `packages/domain/src/companyAliases.js`（`eb3b766`）迁入；准入纪律不变：
//! ADR 映射必须经真实数据源人工核实后才能进表。

use fancy_regex::Regex;
use std::sync::LazyLock;

#[derive(Clone, Debug)]
pub struct CompanyAlias {
    pub pattern: Regex,
    pub ticker: &'static str,
    pub name: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HkUsLinkKind {
    DualPrimary,
    AdrOtc,
}

#[derive(Clone, Debug)]
pub struct HkUsLink {
    pub name_zh: &'static str,
    pub hk: &'static str,
    pub us: Option<&'static str>,
    pub adr: &'static str,
    pub kind: HkUsLinkKind,
    pub pattern: Option<Regex>,
}

fn re(pattern: &str) -> Regex {
    Regex::new(&format!("(?i){pattern}"))
        .unwrap_or_else(|error| panic!("invalid company alias pattern `{pattern}`: {error}"))
}
pub static HK_COMPANY_ALIASES: LazyLock<Vec<CompanyAlias>> = LazyLock::new(|| {
    vec![
        CompanyAlias {
            pattern: re("腾讯控股|腾讯|Tencent"),
            ticker: "0700.HK",
            name: None,
        },
        CompanyAlias {
            pattern: re("阿里巴巴|阿里(?!健康|影业)|Alibaba"),
            ticker: "9988.HK",
            name: None,
        },
        CompanyAlias {
            pattern: re("阿里健康"),
            ticker: "0241.HK",
            name: None,
        },
        CompanyAlias {
            pattern: re("阿里影业"),
            ticker: "1060.HK",
            name: None,
        },
        CompanyAlias {
            pattern: re("美团|Meituan"),
            ticker: "3690.HK",
            name: None,
        },
        CompanyAlias {
            pattern: re("小米|Xiaomi"),
            ticker: "1810.HK",
            name: None,
        },
        CompanyAlias {
            pattern: re("比亚迪|BYD(?![A-Z])"),
            ticker: "1211.HK",
            name: None,
        },
        CompanyAlias {
            pattern: re("京东(?!方)|JD\\.com|jingdong"),
            ticker: "9618.HK",
            name: None,
        },
        CompanyAlias {
            pattern: re("百度|Baidu"),
            ticker: "9888.HK",
            name: None,
        },
        CompanyAlias {
            pattern: re("快手|Kuaishou"),
            ticker: "1024.HK",
            name: None,
        },
        CompanyAlias {
            pattern: re("网易|NetEase"),
            ticker: "9999.HK",
            name: None,
        },
        CompanyAlias {
            pattern: re("联想|Lenovo"),
            ticker: "0992.HK",
            name: None,
        },
        CompanyAlias {
            pattern: re("耐世特|Nexteer"),
            ticker: "1316.HK",
            name: None,
        },
        CompanyAlias {
            pattern: re("地平线机器人|地平线|Horizon Robotics"),
            ticker: "9660.HK",
            name: None,
        },
        CompanyAlias {
            pattern: re("港交所|香港交易所|HKEX(?![A-Za-z])"),
            ticker: "0388.HK",
            name: None,
        },
    ]
});

pub static US_COMPANY_ALIASES: LazyLock<Vec<CompanyAlias>> = LazyLock::new(|| {
    vec![
        CompanyAlias {
            pattern: re("苹果|Apple|\\bAAPL\\b"),
            ticker: "AAPL",
            name: Some("苹果 Apple"),
        },
        CompanyAlias {
            pattern: re("英伟达|NVIDIA|\\bNVDA\\b"),
            ticker: "NVDA",
            name: Some("英伟达 NVIDIA"),
        },
        CompanyAlias {
            pattern: re("特斯拉|Tesla|\\bTSLA\\b"),
            ticker: "TSLA",
            name: Some("特斯拉 Tesla"),
        },
        CompanyAlias {
            pattern: re("微软|Microsoft|\\bMSFT\\b"),
            ticker: "MSFT",
            name: Some("微软 Microsoft"),
        },
        CompanyAlias {
            pattern: re("谷歌|Google|Alphabet|\\bGOOGL?\\b"),
            ticker: "GOOGL",
            name: Some("谷歌 Alphabet"),
        },
        CompanyAlias {
            pattern: re("亚马逊|Amazon|\\bAMZN\\b"),
            ticker: "AMZN",
            name: Some("亚马逊 Amazon"),
        },
        CompanyAlias {
            pattern: re("\\bMeta\\b|Facebook|\\bMETA\\b"),
            ticker: "META",
            name: Some("Meta"),
        },
        CompanyAlias {
            pattern: re("奈飞|网飞|Netflix|\\bNFLX\\b"),
            ticker: "NFLX",
            name: Some("奈飞 Netflix"),
        },
        CompanyAlias {
            pattern: re("英特尔|Intel|\\bINTC\\b"),
            ticker: "INTC",
            name: Some("英特尔 Intel"),
        },
        CompanyAlias {
            pattern: re("\\bAMD\\b|超威"),
            ticker: "AMD",
            name: Some("AMD"),
        },
        CompanyAlias {
            pattern: re("台积电|TSMC|\\bTSM\\b"),
            ticker: "TSM",
            name: Some("台积电 TSMC"),
        },
        CompanyAlias {
            pattern: re("美光|镁光|Micron|\\bMU\\b"),
            ticker: "MU",
            name: Some("美光科技 Micron"),
        },
        CompanyAlias {
            pattern: re("博通|Broadcom|\\bAVGO\\b"),
            ticker: "AVGO",
            name: Some("博通 Broadcom"),
        },
        CompanyAlias {
            pattern: re("高通|Qualcomm|\\bQCOM\\b"),
            ticker: "QCOM",
            name: Some("高通 Qualcomm"),
        },
        CompanyAlias {
            pattern: re("阿斯麦|阿斯麦尔|\\bASML\\b"),
            ticker: "ASML",
            name: Some("阿斯麦 ASML"),
        },
        CompanyAlias {
            pattern: re("应用材料|Applied Materials|\\bAMAT\\b"),
            ticker: "AMAT",
            name: Some("应用材料 Applied Materials"),
        },
        CompanyAlias {
            pattern: re("美满|Marvell|\\bMRVL\\b"),
            ticker: "MRVL",
            name: Some("美满电子 Marvell"),
        },
        CompanyAlias {
            pattern: re("\\bARM\\b|安谋"),
            ticker: "ARM",
            name: Some("ARM"),
        },
        CompanyAlias {
            pattern: re("甲骨文|Oracle|\\bORCL\\b"),
            ticker: "ORCL",
            name: Some("甲骨文 Oracle"),
        },
        CompanyAlias {
            pattern: re("思科|Cisco|\\bCSCO\\b"),
            ticker: "CSCO",
            name: Some("思科 Cisco"),
        },
        CompanyAlias {
            pattern: re("Adobe|\\bADBE\\b"),
            ticker: "ADBE",
            name: Some("Adobe"),
        },
        CompanyAlias {
            pattern: re("Salesforce|赛富时|\\bCRM\\b"),
            ticker: "CRM",
            name: Some("Salesforce"),
        },
        CompanyAlias {
            pattern: re("Palantir|\\bPLTR\\b"),
            ticker: "PLTR",
            name: Some("Palantir"),
        },
        CompanyAlias {
            pattern: re("Snowflake|\\bSNOW\\b"),
            ticker: "SNOW",
            name: Some("Snowflake"),
        },
        CompanyAlias {
            pattern: re("Coinbase|\\bCOIN\\b"),
            ticker: "COIN",
            name: Some("Coinbase"),
        },
        CompanyAlias {
            pattern: re("优步|Uber|\\bUBER\\b"),
            ticker: "UBER",
            name: Some("优步 Uber"),
        },
        CompanyAlias {
            pattern: re("迪士尼|Disney|\\bDIS\\b"),
            ticker: "DIS",
            name: Some("迪士尼 Disney"),
        },
        CompanyAlias {
            pattern: re("星巴克|Starbucks|\\bSBUX\\b"),
            ticker: "SBUX",
            name: Some("星巴克 Starbucks"),
        },
        CompanyAlias {
            pattern: re("麦当劳|McDonald|\\bMCD\\b"),
            ticker: "MCD",
            name: Some("麦当劳 McDonald's"),
        },
        CompanyAlias {
            pattern: re("可口可乐|Coca[ -]?Cola"),
            ticker: "KO",
            name: Some("可口可乐 Coca-Cola"),
        },
        CompanyAlias {
            pattern: re("百事|Pepsi|\\bPEP\\b"),
            ticker: "PEP",
            name: Some("百事 PepsiCo"),
        },
        CompanyAlias {
            pattern: re("沃尔玛|Walmart|\\bWMT\\b"),
            ticker: "WMT",
            name: Some("沃尔玛 Walmart"),
        },
        CompanyAlias {
            pattern: re("耐克|Nike"),
            ticker: "NKE",
            name: Some("耐克 Nike"),
        },
        CompanyAlias {
            pattern: re("波音|Boeing"),
            ticker: "BA",
            name: Some("波音 Boeing"),
        },
        CompanyAlias {
            pattern: re("摩根大通|小摩|JPMorgan|JP\\s?Morgan|\\bJPM\\b"),
            ticker: "JPM",
            name: Some("摩根大通 JPMorgan"),
        },
        CompanyAlias {
            pattern: re("高盛|Goldman"),
            ticker: "GS",
            name: Some("高盛 Goldman Sachs"),
        },
        CompanyAlias {
            pattern: re("伯克希尔|巴菲特|Berkshire"),
            ticker: "BRK-B",
            name: Some("伯克希尔 Berkshire"),
        },
        CompanyAlias {
            pattern: re("Visa|维萨"),
            ticker: "V",
            name: Some("Visa"),
        },
        CompanyAlias {
            pattern: re("万事达|Mastercard"),
            ticker: "MA",
            name: Some("万事达 Mastercard"),
        },
        CompanyAlias {
            pattern: re("礼来|Eli\\s?Lilly|\\bLLY\\b"),
            ticker: "LLY",
            name: Some("礼来 Eli Lilly"),
        },
        CompanyAlias {
            pattern: re("强生|Johnson\\s?&?\\s?Johnson|\\bJNJ\\b"),
            ticker: "JNJ",
            name: Some("强生 J&J"),
        },
        CompanyAlias {
            pattern: re("辉瑞|Pfizer|\\bPFE\\b"),
            ticker: "PFE",
            name: Some("辉瑞 Pfizer"),
        },
        CompanyAlias {
            pattern: re("\\bBABA\\b"),
            ticker: "BABA",
            name: Some("阿里巴巴 ADR"),
        },
    ]
});

pub static HK_US_LINKS: LazyLock<Vec<HkUsLink>> = LazyLock::new(|| {
    vec![
        HkUsLink {
            name_zh: "阿里巴巴",
            hk: "9988.HK",
            us: Some("BABA"),
            adr: "BABA",
            kind: HkUsLinkKind::DualPrimary,
            pattern: Some(re("阿里巴巴|阿里(?!健康|影业)|Alibaba")),
        },
        HkUsLink {
            name_zh: "京东",
            hk: "9618.HK",
            us: Some("JD"),
            adr: "JD",
            kind: HkUsLinkKind::DualPrimary,
            pattern: Some(re("京东(?!方)|JD\\.com|jingdong")),
        },
        HkUsLink {
            name_zh: "百度",
            hk: "9888.HK",
            us: Some("BIDU"),
            adr: "BIDU",
            kind: HkUsLinkKind::DualPrimary,
            pattern: Some(re("百度|Baidu")),
        },
        HkUsLink {
            name_zh: "网易",
            hk: "9999.HK",
            us: Some("NTES"),
            adr: "NTES",
            kind: HkUsLinkKind::DualPrimary,
            pattern: Some(re("网易|NetEase")),
        },
        HkUsLink {
            name_zh: "携程",
            hk: "9961.HK",
            us: Some("TCOM"),
            adr: "TCOM",
            kind: HkUsLinkKind::DualPrimary,
            pattern: Some(re("携程|Trip\\.com|ctrip")),
        },
        HkUsLink {
            name_zh: "哔哩哔哩",
            hk: "9626.HK",
            us: Some("BILI"),
            adr: "BILI",
            kind: HkUsLinkKind::DualPrimary,
            pattern: Some(re("哔哩哔哩|bilibili")),
        },
        HkUsLink {
            name_zh: "理想汽车",
            hk: "2015.HK",
            us: Some("LI"),
            adr: "LI",
            kind: HkUsLinkKind::DualPrimary,
            pattern: Some(re("理想汽车|Li\\s?Auto")),
        },
        HkUsLink {
            name_zh: "小鹏汽车",
            hk: "9868.HK",
            us: Some("XPEV"),
            adr: "XPEV",
            kind: HkUsLinkKind::DualPrimary,
            pattern: Some(re("小鹏|XPeng")),
        },
        HkUsLink {
            name_zh: "蔚来",
            hk: "9866.HK",
            us: Some("NIO"),
            adr: "NIO",
            kind: HkUsLinkKind::DualPrimary,
            pattern: Some(re("蔚来|\\bNIO\\b")),
        },
        HkUsLink {
            name_zh: "名创优品",
            hk: "9896.HK",
            us: Some("MNSO"),
            adr: "MNSO",
            kind: HkUsLinkKind::DualPrimary,
            pattern: Some(re("名创优品|Miniso")),
        },
        HkUsLink {
            name_zh: "新东方",
            hk: "9901.HK",
            us: Some("EDU"),
            adr: "EDU",
            kind: HkUsLinkKind::DualPrimary,
            pattern: Some(re("新东方|New\\s?Oriental")),
        },
        HkUsLink {
            name_zh: "贝壳",
            hk: "2423.HK",
            us: Some("BEKE"),
            adr: "BEKE",
            kind: HkUsLinkKind::DualPrimary,
            pattern: Some(re("贝壳|Beike|KE\\s?Holdings")),
        },
        HkUsLink {
            name_zh: "腾讯控股",
            hk: "0700.HK",
            us: None,
            adr: "TCEHY",
            kind: HkUsLinkKind::AdrOtc,
            pattern: None,
        },
        HkUsLink {
            name_zh: "美团",
            hk: "3690.HK",
            us: None,
            adr: "MPNGY",
            kind: HkUsLinkKind::AdrOtc,
            pattern: None,
        },
        HkUsLink {
            name_zh: "小米集团",
            hk: "1810.HK",
            us: None,
            adr: "XIACY",
            kind: HkUsLinkKind::AdrOtc,
            pattern: None,
        },
        HkUsLink {
            name_zh: "中国平安",
            hk: "2318.HK",
            us: None,
            adr: "PNGAY",
            kind: HkUsLinkKind::AdrOtc,
            pattern: None,
        },
        HkUsLink {
            name_zh: "比亚迪",
            hk: "1211.HK",
            us: None,
            adr: "BYDDY",
            kind: HkUsLinkKind::AdrOtc,
            pattern: None,
        },
        HkUsLink {
            name_zh: "汇丰控股",
            hk: "0005.HK",
            us: None,
            adr: "HSBC",
            kind: HkUsLinkKind::AdrOtc,
            pattern: None,
        },
        HkUsLink {
            name_zh: "中国移动",
            hk: "0941.HK",
            us: None,
            adr: "CHL",
            kind: HkUsLinkKind::AdrOtc,
            pattern: None,
        },
    ]
});

fn hk_code_of(ticker: &str) -> String {
    let value = ticker.trim().to_ascii_uppercase();
    let code = value.strip_suffix(".HK").unwrap_or(&value);
    format!("{code:0>4}")
}

/// 仅真双重上市（需要问用户市场口径的那类）。
#[must_use]
pub fn dual_listings() -> Vec<&'static HkUsLink> {
    HK_US_LINKS
        .iter()
        .filter(|link| link.kind == HkUsLinkKind::DualPrimary)
        .collect()
}

/// 任一腿代码 → 双重上市条目（仅 dual_primary）。
#[must_use]
pub fn dual_listing_by_ticker(ticker: &str) -> Option<&'static HkUsLink> {
    let key = ticker.trim().to_ascii_uppercase();
    HK_US_LINKS.iter().find(|link| {
        link.kind == HkUsLinkKind::DualPrimary
            && (link.hk == key || link.us.is_some_and(|us| us == key))
    })
}

/// 问句里点名的双重上市公司。
#[must_use]
pub fn dual_listing_by_name(text: &str) -> Option<&'static HkUsLink> {
    HK_US_LINKS.iter().find(|link| {
        link.kind == HkUsLinkKind::DualPrimary
            && link
                .pattern
                .as_ref()
                .is_some_and(|pattern| pattern.is_match(text).unwrap_or(false))
    })
}

/// 港股代码 → Finnhub 可查询的美股 ADR 替身；无核实条目即 `None`。
#[must_use]
pub fn adr_for_hk(ticker: &str) -> Option<&'static str> {
    let code = hk_code_of(ticker);
    HK_US_LINKS
        .iter()
        .find(|link| hk_code_of(link.hk) == code)
        .map(|link| link.adr)
}

#[must_use]
pub fn match_hk_alias(text: &str) -> Option<&'static CompanyAlias> {
    HK_COMPANY_ALIASES
        .iter()
        .find(|item| item.pattern.is_match(text).unwrap_or(false))
}

#[must_use]
pub fn match_us_alias(text: &str) -> Option<&'static CompanyAlias> {
    US_COMPANY_ALIASES
        .iter()
        .find(|item| item.pattern.is_match(text).unwrap_or(false))
}

/// 问句里点名的一家公司——纯规则命中（别名或显式代码）；`position` 是它在归一化
/// 文本里的字节位，用于按出现顺序排主体/对照。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompanyMention {
    pub ticker: String,
    pub position: usize,
}

/// 同一家公司跨上市地的归并键——双重上市（如 9988.HK / BABA）折叠为港股腿，
/// 避免"阿里巴巴"同时命中两张别名表被误判成两家公司。
#[must_use]
pub fn company_identity_key(ticker: &str) -> String {
    let key = ticker.trim().to_ascii_uppercase();
    dual_listing_by_ticker(&key).map_or(key, |link| link.hk.to_string())
}

/// 问句里点名的全部公司（跨港/美别名表 + 显式代码），按出现位置排序、按公司归并键
/// 去重。纯规则识别，结果仍需供应商验证后才能研究；命中两家及以上即多主体候选。
#[must_use]
pub fn match_company_mentions(text: &str) -> Vec<CompanyMention> {
    let raw = crate::company_identity::normalize_question_text(text);
    let mut hits: Vec<CompanyMention> = Vec::new();
    for alias in HK_COMPANY_ALIASES.iter().chain(US_COMPANY_ALIASES.iter()) {
        if let Ok(Some(found)) = alias.pattern.find(&raw) {
            hits.push(CompanyMention {
                ticker: alias.ticker.to_string(),
                position: found.start(),
            });
        }
    }
    // 显式代码（1316.HK / $RKLB 之类）——别名表没覆盖的公司也能进多主体识别。
    // "vs/PK" 是对比连接词，不许被裸词元抽取误当美股代码。
    if let Some(hk) = crate::company_identity::extract_hk_ticker(&raw) {
        let digits = hk.trim_end_matches(".HK").trim_start_matches('0');
        let position = raw.find(digits).unwrap_or(raw.len());
        hits.push(CompanyMention {
            ticker: hk,
            position,
        });
    }
    if let Some(us) = crate::company_identity::extract_us_ticker_token(&raw, &["VS", "PK"]) {
        let position = raw.to_ascii_uppercase().find(&us).unwrap_or(raw.len());
        hits.push(CompanyMention {
            ticker: us,
            position,
        });
    }
    hits.sort_by_key(|hit| hit.position);
    let mut seen = std::collections::HashSet::new();
    hits.retain(|hit| seen.insert(company_identity_key(&hit.ticker)));
    hits
}

/// 问句是否带对比语气——多主体自动对比的第二道门：只点名两家公司不必然是对比
/// （"用微软的打法分析苹果"仍是单主体研究）。
#[must_use]
pub fn has_compare_cue(text: &str) -> bool {
    static CUE: LazyLock<Regex> = LazyLock::new(|| {
        re(
            r"对比|比较|相比|比起|对标|谁更|谁(比较)?(强|贵|便宜|高|低|好|优)|谁的.{0,6}(好|强|高|低|贵|便宜)|哪个|哪家|哪只|孰|还是|\bvs\.?\b|\bpk\b|versus|更(好|强|贵|便宜|值得|优)|[和与跟].{1,12}比",
        )
    });
    CUE.is_match(text).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_tables_hit_english_and_chinese_names() {
        assert_eq!(match_hk_alias("tencent").map(|a| a.ticker), Some("0700.HK"));
        assert_eq!(match_hk_alias("alibaba").map(|a| a.ticker), Some("9988.HK"));
        assert_eq!(
            match_hk_alias("阿里健康怎么样").map(|a| a.ticker),
            Some("0241.HK")
        );
        assert_eq!(match_us_alias("nvidia").map(|a| a.ticker), Some("NVDA"));
        assert_eq!(match_us_alias("英伟达").map(|a| a.ticker), Some("NVDA"));
        assert_eq!(match_us_alias("BABA").map(|a| a.ticker), Some("BABA"));
    }

    #[test]
    fn dual_listing_and_adr_helpers_match_baseline() {
        assert_eq!(
            dual_listing_by_ticker("9988.HK").map(|l| l.us),
            Some(Some("BABA"))
        );
        assert_eq!(
            dual_listing_by_ticker("BABA").map(|l| l.hk),
            Some("9988.HK")
        );
        assert!(dual_listing_by_ticker("0700.HK").is_none());
        assert_eq!(
            dual_listing_by_name("alibaba最近怎么样").map(|l| l.hk),
            Some("9988.HK")
        );
        assert_eq!(
            dual_listing_by_name("bilibili还在亏吗").map(|l| l.us),
            Some(Some("BILI"))
        );
        assert!(dual_listing_by_name("腾讯怎么样").is_none());
        assert_eq!(adr_for_hk("0700.HK"), Some("TCEHY"));
        assert_eq!(adr_for_hk("700"), Some("TCEHY"));
        assert_eq!(adr_for_hk("9988.HK"), Some("BABA"));
        assert_eq!(adr_for_hk("1024.HK"), None);
        assert!(
            dual_listings()
                .iter()
                .all(|l| l.kind == HkUsLinkKind::DualPrimary)
        );
    }

    #[test]
    fn company_mentions_find_two_distinct_companies_in_order() {
        let mentions = match_company_mentions("苹果和微软谁更值得买");
        let tickers: Vec<&str> = mentions.iter().map(|m| m.ticker.as_str()).collect();
        assert_eq!(tickers, vec!["AAPL", "MSFT"]);

        let mentions = match_company_mentions("腾讯 vs 网易，游戏业务谁强");
        let tickers: Vec<&str> = mentions.iter().map(|m| m.ticker.as_str()).collect();
        // "vs" 是连接词，绝不许被当成美股代码；网易命中一次不重复。
        assert_eq!(tickers, vec!["0700.HK", "9999.HK"]);
    }

    #[test]
    fn dual_listing_mention_is_one_company_not_two() {
        // "阿里巴巴" 同时命中港/美两张别名表——必须按公司归并键折叠成一家。
        let mentions = match_company_mentions("阿里巴巴现在便宜吗");
        assert_eq!(mentions.len(), 1);
        assert_eq!(
            company_identity_key(&mentions[0].ticker),
            company_identity_key("BABA")
        );
    }

    #[test]
    fn compare_cue_gates_multi_subject_questions() {
        assert!(has_compare_cue("苹果和微软谁更值得买"));
        assert!(has_compare_cue("苹果和微软谁贵"));
        assert!(has_compare_cue("腾讯和阿里谁便宜"));
        assert!(has_compare_cue("腾讯 vs 网易"));
        assert!(has_compare_cue("英伟达对比 AMD 的估值"));
        assert!(has_compare_cue("美团和京东比毛利率"));
        assert!(!has_compare_cue("英伟达的护城河在哪"));
        assert!(!has_compare_cue("用微软的打法分析苹果"));
    }
}
