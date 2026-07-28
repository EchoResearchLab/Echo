//! Echo Research 的 Cargo 单入口工程任务。
//!
//! 统一执行 Rust 格式化、静态检查、测试、数据库迁移与 WASM 构建。

use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

mod frozen;

type TaskResult<T = ()> = Result<T, String>;

fn workspace_root() -> TaskResult<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "xtask 必须位于 workspace/xtask".to_string())
}

fn run<I, S>(program: &str, args: I, cwd: &Path) -> TaskResult
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args: Vec<_> = args
        .into_iter()
        .map(|arg| arg.as_ref().to_owned())
        .collect();
    eprintln!(
        "+ (cd {} && {} {})",
        cwd.display(),
        program,
        args.iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    );
    let status = Command::new(program)
        .args(&args)
        .current_dir(cwd)
        // Trunk 0.21 把 NO_COLOR 当 bool 解析，而部分宿主注入惯用值 "1"；移除后使用默认彩色策略。
        .env_remove("NO_COLOR")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|error| format!("无法启动 {program}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} 退出状态 {status}"))
    }
}

fn rust_checks(root: &Path) -> TaskResult {
    // 冻结检查排在最前：它不编译任何东西，几毫秒就能拦住"建了表没人调"这类
    // 编译器永远不会抱怨、但会一路带进生产的缺陷。
    frozen::check(root)?;
    run("cargo", ["fmt", "--all", "--", "--check"], root)?;
    run(
        "cargo",
        [
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
        root,
    )?;
    run("cargo", ["test", "--workspace"], root)
}

fn web_build(root: &Path, release: bool) -> TaskResult {
    let web = root.join("crates/frontend/echo-web");
    if release {
        run("trunk", ["build", "--release"], &web)
    } else {
        run("trunk", ["build"], &web)
    }
}

fn browser_e2e(root: &Path) -> TaskResult {
    run(
        "cargo",
        ["test", "-p", "echo-e2e", "--", "--ignored", "--nocapture"],
        root,
    )
}

fn migrate_database() -> TaskResult {
    let database_url = env::var("DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "migrate 需要显式 DATABASE_URL".to_string())?;
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|error| format!("创建 tokio runtime 失败: {error}"))?;
    runtime.block_on(async move {
        let pool = echo_db::connect(&database_url, 1)
            .await
            .map_err(|error| error.to_string())?;
        let applied = echo_db::migrate(&pool)
            .await
            .map_err(|error| error.to_string())?;
        for name in applied {
            eprintln!("[db:migrate] applied {name}");
        }
        Ok(())
    })
}

fn bootstrap_owner() -> TaskResult {
    let database_url = env::var("DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "bootstrap-owner 需要显式 DATABASE_URL".to_string())?;
    let email = env::var("ECHO_BOOTSTRAP_EMAIL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "bootstrap-owner 需要 ECHO_BOOTSTRAP_EMAIL".to_string())?;
    let password = env::var("ECHO_BOOTSTRAP_PASSWORD")
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "bootstrap-owner 需要 ECHO_BOOTSTRAP_PASSWORD".to_string())?;
    let display_name = env::var("ECHO_BOOTSTRAP_DISPLAY_NAME")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|error| format!("创建 tokio runtime 失败: {error}"))?;
    runtime.block_on(async move {
        let pool = echo_db::connect(&database_url, 1)
            .await
            .map_err(|error| error.to_string())?;
        echo_db::migrate(&pool)
            .await
            .map_err(|error| error.to_string())?;
        let user = echo_application::AuthService::new(&pool)
            .create_owner(&email, &password, display_name)
            .await
            .map_err(|error| error.to_string())?;
        eprintln!("[auth] owner {} 已创建", user.username);
        Ok(())
    })
}

fn ingest_hk_financials(path: &Path) -> TaskResult {
    let database_url = env::var("DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "hk-ingest 需要显式 DATABASE_URL".to_string())?;
    let body = std::fs::read_to_string(path)
        .map_err(|error| format!("读取 {} 失败: {error}", path.display()))?;
    let raw: echo_data::RawHkFinancials = serde_json::from_str(&body)
        .map_err(|error| format!("解析 {} 失败: {error}", path.display()))?;
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|error| format!("创建 tokio runtime 失败: {error}"))?;
    runtime.block_on(async move {
        let pool = echo_db::connect(&database_url, 1)
            .await
            .map_err(|error| error.to_string())?;
        echo_db::migrate(&pool)
            .await
            .map_err(|error| error.to_string())?;
        let normalized = echo_data::ingest_hk_financials(&pool, raw)
            .await
            .map_err(|error| error.to_string())?;
        eprintln!(
            "[hk-ingest] {} {} 已写入；来源倍率={}，解析器={}",
            normalized.ticker,
            normalized.period_label.as_deref().unwrap_or("期间未标"),
            normalized.source_unit_scale.normalize(),
            normalized.parser_version
        );
        Ok(())
    })
}

fn discover_hk_announcements(ticker: &str) -> TaskResult {
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|error| format!("创建 tokio runtime 失败: {error}"))?;
    runtime.block_on(async move {
        let service = echo_data::HkexService::new().map_err(|error| error.to_string())?;
        let announcements = service
            .results_announcements(ticker, 2, 10)
            .await
            .map_err(|error| error.to_string())?;
        let output = serde_json::to_string_pretty(&announcements)
            .map_err(|error| format!("序列化公告失败: {error}"))?;
        println!("{output}");
        Ok(())
    })
}

fn usage() {
    eprintln!(
        "用法: cargo xtask <命令>\n\n  check              冻结检查 + Rust fmt + clippy + test + Leptos/WASM release build\n  frozen             只跑冻结检查：活表无 Rust 读写、API 路由无前端调用方\n  web                构建开发版 Leptos/WASM\n  e2e                通过 WebDriver 验收真实浏览器核心流程\n  migrate            对显式 DATABASE_URL 执行 Rust 迁移\n  bootstrap-owner    用显式环境变量创建首个 owner（不覆盖已有 owner）\n  hk-discover <代码> 从 HKEX 披露易发现最近业绩公告\n  hk-ingest <json>   校验并写入一份结构化 HKEX 业绩公告\n  release            执行完整 Rust 检查并构建 release Web"
    );
}

fn main() -> ExitCode {
    let root = match workspace_root() {
        Ok(root) => root,
        Err(error) => {
            eprintln!("xtask: {error}");
            return ExitCode::FAILURE;
        }
    };
    let command = env::args().nth(1);
    let result = match command.as_deref() {
        Some("check") | Some("release") => rust_checks(&root).and_then(|()| web_build(&root, true)),
        Some("frozen") => frozen::check(&root),
        Some("web") => web_build(&root, false),
        Some("e2e") => browser_e2e(&root),
        Some("migrate") => migrate_database(),
        Some("bootstrap-owner") => bootstrap_owner(),
        Some("hk-discover") => match env::args().nth(2) {
            Some(ticker) => discover_hk_announcements(&ticker),
            None => Err("hk-discover 需要港股代码".into()),
        },
        Some("hk-ingest") => match env::args().nth(2) {
            Some(path) => ingest_hk_financials(Path::new(&path)),
            None => Err("hk-ingest 需要 JSON 文件路径".into()),
        },
        _ => {
            usage();
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xtask: {error}");
            ExitCode::FAILURE
        }
    }
}
