//! 展示层格式化——只在这里把服务端口径（RFC3339 UTC、原始枚举串、内部指标名）翻成
//! 用户可读文本。业务层不做展示决定，展示层不做业务判断。
//!
//! 时间一律按浏览器本地时区渲染：服务端存的是 UTC，直接把 UTC 数字当本地时间显示会把
//! 港股盘后写成盘中，这在投研产品里是事实错误，不是排版瑕疵。

/// 把 RFC3339 时间戳渲染成紧凑的本地时间：同年 `7月24日 14:55`，跨年 `2025年7月24日`。
/// 解析失败时退回原串的日期部分，绝不显示 `Invalid Date` 之类的技术噪音。
pub fn timestamp(iso: &str) -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(iso));
        let time = date.get_time();
        if time.is_nan() {
            return fallback_date(iso);
        }
        let now = js_sys::Date::new_0();
        let year = date.get_full_year();
        let month = date.get_month() + 1;
        let day = date.get_date();
        if year == now.get_full_year() {
            format!(
                "{month}月{day}日 {:02}:{:02}",
                date.get_hours(),
                date.get_minutes()
            )
        } else {
            format!("{year}年{month}月{day}日")
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fallback_date(iso)
    }
}

/// 只取日期部分（`YYYY-MM-DD`）作为兜底展示。
fn fallback_date(iso: &str) -> String {
    iso.split('T').next().unwrap_or(iso).to_string()
}

/// 研究深度的中文标签——`route.depth` 是接口层的英文枚举串，不直接进界面。
pub fn depth_label(depth: &str) -> &str {
    match depth {
        "brief" => "速答",
        "deep" => "深度",
        "standard" => "标准",
        other => other,
    }
}

/// 基本面指标的中文标签。服务端用驼峰字段名标识指标，用户界面必须给可读名称；
/// 未收录的指标原样透出，好过显示一个错的中文名。
pub fn metric_label(metric: &str) -> &str {
    match metric {
        "revenueGrowth" => "营收同比增速",
        "grossMargin" => "毛利率",
        "operatingMargin" => "经营利润率",
        "netMargin" => "净利率",
        "freeCashFlow" => "自由现金流",
        "peRatio" => "市盈率",
        "pbRatio" => "市净率",
        "roe" => "净资产收益率",
        "debtToEquity" => "资产负债结构",
        "eps" => "每股收益",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_to_date_part_on_native_target() {
        assert_eq!(timestamp("2026-07-24T06:55:18.812646+00:00"), "2026-07-24");
    }

    #[test]
    fn unmapped_labels_pass_through_rather_than_guessing() {
        assert_eq!(depth_label("standard"), "标准");
        assert_eq!(depth_label("unknown_depth"), "unknown_depth");
        assert_eq!(metric_label("grossMargin"), "毛利率");
        assert_eq!(metric_label("customMetric"), "customMetric");
    }
}
