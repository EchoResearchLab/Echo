//! Rust 服务的统一日志/追踪入口。

use std::sync::OnceLock;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::{SdkTracerProvider, span_processor_with_async_runtime};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer as _;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// 进程内唯一的 OTLP TracerProvider——`init` 时按需建一次，`shutdown` 时排空批处理队列。
/// 未配置 `OTEL_EXPORTER_OTLP_ENDPOINT` 时始终是 `None`（完全不建 provider，不后台起
/// 任何导出线程），不是"配了却导出失败"的静默降级，是真正的"没配就不跑"。
static TRACER_PROVIDER: OnceLock<SdkTracerProvider> = OnceLock::new();

/// 初始化全局 subscriber。`RUST_LOG` 控制过滤；`ECHO_LOG_FORMAT=json` 输出结构化日志；
/// `OTEL_EXPORTER_OTLP_ENDPOINT`（标准 OTel 环境变量名，值形如 `http://localhost:4318`，
/// 只接受明文 HTTP——收集端通常是同网/同机 sidecar，HTTPS 收集端需要额外 TLS 后端选型，
/// 不在本次范围内）非空时额外挂一层 OTLP span 导出，与既有 stdout 日志并行、互不替代。
pub fn init(service: &'static str) -> Result<(), String> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let json =
        std::env::var("ECHO_LOG_FORMAT").is_ok_and(|value| value.eq_ignore_ascii_case("json"));
    let fmt_layer = if json {
        tracing_subscriber::fmt::layer()
            .with_target(true)
            .json()
            .with_current_span(true)
            .with_span_list(true)
            .boxed()
    } else {
        tracing_subscriber::fmt::layer()
            .with_target(true)
            .compact()
            .boxed()
    };

    let otlp_endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    match otlp_endpoint {
        None => tracing_subscriber::registry()
            .with(filter)
            .with(fmt_layer)
            .try_init()
            .map_err(|error| format!("初始化 {service} tracing 失败: {error}")),
        Some(endpoint) => {
            let traces_url = format!("{}/v1/traces", endpoint.trim_end_matches('/'));
            let exporter = opentelemetry_otlp::SpanExporter::builder()
                .with_http()
                .with_endpoint(&traces_url)
                .build()
                .map_err(|error| format!("构造 {service} OTLP 导出器失败: {error}"))?;
            let processor = span_processor_with_async_runtime::BatchSpanProcessor::builder(
                exporter,
                opentelemetry_sdk::runtime::Tokio,
            )
            .build();
            let resource = Resource::builder().with_service_name(service).build();
            let provider = SdkTracerProvider::builder()
                .with_resource(resource)
                .with_span_processor(processor)
                .build();
            let otel_layer = tracing_opentelemetry::layer().with_tracer(provider.tracer(service));

            tracing_subscriber::registry()
                .with(filter)
                .with(fmt_layer)
                .with(otel_layer)
                .try_init()
                .map_err(|error| format!("初始化 {service} tracing 失败: {error}"))?;

            TRACER_PROVIDER
                .set(provider)
                .map_err(|_| "TracerProvider 重复初始化".to_string())?;
            tracing::info!(endpoint = %traces_url, "OTLP 追踪导出已启用");
            Ok(())
        }
    }
}

/// 优雅停机时调用：排空 OTLP 批处理队列里未发送的 span，避免进程退出丢尾部追踪数据。
/// 未配置 OTLP 时是纯粹的空操作（`TRACER_PROVIDER` 从未被写入）。
pub fn shutdown() {
    if let Some(provider) = TRACER_PROVIDER.get() {
        if let Err(error) = provider.shutdown() {
            tracing::warn!(error = %error, "OTLP TracerProvider 停机排空失败");
        }
    }
}
