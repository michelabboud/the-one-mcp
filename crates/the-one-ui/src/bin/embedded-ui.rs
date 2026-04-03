use std::sync::Arc;

use the_one_mcp::broker::McpBroker;
use the_one_ui::{resolve_ui_runtime_config, start_embedded_ui_runtime, AdminUi};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let cwd = std::env::current_dir().expect("cwd should be available");
    let resolved = resolve_ui_runtime_config(&cwd).expect("ui runtime config should resolve");
    let bind_addr = resolved
        .bind_addr
        .parse()
        .expect("THE_ONE_UI_BIND/ui_bind must be host:port");

    let admin = Arc::new(AdminUi::new(McpBroker::new()));
    let _ = admin.trigger_project_init(&resolved.project_root, &resolved.project_id);

    let runtime =
        start_embedded_ui_runtime(admin, resolved.project_root, resolved.project_id, bind_addr)
            .await
            .expect("embedded ui should start");
    println!("embedded-ui listening on http://{}", runtime.listen_addr);

    let _ = tokio::signal::ctrl_c().await;
    runtime.shutdown();
}
