use std::sync::Arc;

use clap::{Parser, ValueEnum};

use the_one_mcp::broker::McpBroker;
use the_one_mcp::transport::sse::SseTransport;
use the_one_mcp::transport::stdio::StdioTransport;
use the_one_mcp::transport::stream::StreamableHttpTransport;
use the_one_mcp::transport::Transport;

#[derive(Parser)]
#[command(name = "the-one-mcp", version, about = "The One MCP broker server")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Start the MCP server
    Serve {
        /// Transport protocol to use
        #[arg(long, default_value = "stdio", value_enum)]
        transport: TransportKind,

        /// Port for HTTP-based transports (SSE, stream)
        #[arg(long, default_value = "3000")]
        port: u16,

        /// Project root directory (defaults to current directory)
        #[arg(long)]
        project_root: Option<String>,

        /// Project identifier (auto-detected if not specified)
        #[arg(long)]
        project_id: Option<String>,
    },
}

#[derive(Clone, ValueEnum)]
enum TransportKind {
    Stdio,
    Sse,
    Stream,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    let _ = the_one_core::telemetry::init_telemetry("info", false);

    let cli = Cli::parse();

    match cli.command {
        Commands::Serve {
            transport,
            port,
            project_root,
            project_id,
        } => {
            let broker = Arc::new(McpBroker::new());

            // If project_root specified, initialize the project
            if let (Some(root), Some(id)) = (&project_root, &project_id) {
                let _ = broker
                    .project_init(the_one_mcp::api::ProjectInitRequest {
                        project_root: root.clone(),
                        project_id: id.clone(),
                    })
                    .await;
            }

            let transport: Box<dyn Transport> = match transport {
                TransportKind::Stdio => {
                    eprintln!("the-one-mcp: starting stdio transport");
                    Box::new(StdioTransport)
                }
                TransportKind::Sse => {
                    eprintln!("the-one-mcp: starting SSE transport on port {port}");
                    Box::new(SseTransport { port })
                }
                TransportKind::Stream => {
                    eprintln!("the-one-mcp: starting streamable HTTP transport on port {port}");
                    Box::new(StreamableHttpTransport { port })
                }
            };

            transport.run(broker).await?;
        }
    }

    Ok(())
}
