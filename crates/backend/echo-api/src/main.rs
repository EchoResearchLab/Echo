fn main() {
    let runtime = tokio::runtime::Runtime::new().expect("create tokio runtime");
    runtime.block_on(echo_api::run());
}
