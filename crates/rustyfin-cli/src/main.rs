use clap::{Parser, Subcommand};
use reqwest::Client;
use serde_json::Value;

/// Rustyfin CLI – call every API endpoint from the command line.
#[derive(Parser)]
#[command(name = "rustyfin-cli", version, about)]
struct Cli {
    /// Base URL of the Rustyfin server (e.g. http://localhost:8096)
    #[arg(short, long, env = "RUSTYFIN_URL", default_value = "http://localhost:8096")]
    url: String,

    /// Auth token (returned by `auth login`). Not needed for public endpoints.
    #[arg(short, long, env = "RUSTYFIN_TOKEN")]
    token: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    // ── System ───────────────────────────────────────────────
    /// Public server info
    SystemInfoPublic,
    /// Ping the server (GET)
    Ping,
    /// Health check
    Health,
    /// Full server info (auth required)
    SystemInfo,
    /// Get server configuration (auth required)
    SystemConfigGet,
    /// Update server configuration from JSON string (auth required)
    SystemConfigUpdate {
        /// JSON body
        json: String,
    },
    /// Shutdown the server (auth required)
    SystemShutdown,
    /// Restart the server (auth required)
    SystemRestart,
    /// List log files (auth required)
    SystemLogs,

    // ── Auth ─────────────────────────────────────────────────
    /// Authenticate by username & password, returns a token
    Login {
        #[arg(short = 'U', long)]
        username: String,
        #[arg(short = 'P', long)]
        password: String,
    },

    // ── Users ────────────────────────────────────────────────
    /// List public users
    UsersPublic,
    /// List all users (auth required)
    Users,
    /// Get current user (auth required)
    UsersMe,
    /// Get user by id (auth required)
    UserGet {
        id: String,
    },
    /// Create a new user (auth required)
    UserCreate {
        #[arg(short = 'U', long)]
        username: String,
        #[arg(short = 'P', long)]
        password: String,
    },
    /// Delete a user (auth required)
    UserDelete {
        id: String,
    },
    /// Change a user's password (auth required)
    UserChangePassword {
        id: String,
        #[arg(long)]
        current_pw: String,
        #[arg(long)]
        new_pw: String,
    },
    /// Update user policy (auth required)
    UserUpdatePolicy {
        id: String,
        /// JSON body, e.g. '{"IsAdministrator": true}'
        json: String,
    },
    /// Update user configuration (auth required)
    UserUpdateConfig {
        id: String,
        /// JSON body
        json: String,
    },
    /// Forgot password
    ForgotPassword {
        username: String,
    },

    // ── Library ──────────────────────────────────────────────
    /// List virtual folders / libraries (auth required)
    Libraries,
    /// Create a library (auth required)
    LibraryCreate {
        #[arg(short, long)]
        name: String,
        /// Content type: Movies, TvShows, Music, Photos, Mixed
        #[arg(short, long)]
        content_type: String,
        /// Comma-separated paths
        #[arg(short, long)]
        paths: String,
    },
    /// Remove a library (auth required)
    LibraryRemove {
        id: String,
    },
    /// Scan / refresh a single library (auth required)
    LibraryScan {
        id: String,
    },
    /// Refresh all libraries (auth required)
    LibraryRefreshAll,

    // ── Items ────────────────────────────────────────────────
    /// Query items (auth required). Supports optional filters.
    Items {
        #[arg(long)]
        parent_id: Option<String>,
        #[arg(long)]
        include_item_types: Option<String>,
        #[arg(long)]
        sort_by: Option<String>,
        #[arg(long)]
        sort_order: Option<String>,
        #[arg(long)]
        start_index: Option<u32>,
        #[arg(long)]
        limit: Option<u32>,
        #[arg(long)]
        recursive: Option<bool>,
        #[arg(long)]
        search_term: Option<String>,
        #[arg(long)]
        genres: Option<String>,
        #[arg(long)]
        years: Option<String>,
    },
    /// Get a single item by id (auth required)
    ItemGet {
        id: String,
    },
    /// Delete an item (auth required)
    ItemDelete {
        id: String,
    },
    /// Get children of an item (auth required)
    ItemChildren {
        id: String,
    },
    /// Get similar items (auth required)
    ItemSimilar {
        id: String,
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Get latest items (auth required)
    ItemsLatest {
        #[arg(long)]
        parent_id: Option<String>,
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Get resume items (auth required)
    ItemsResume,
    /// Get available filters (auth required)
    ItemsFilters {
        #[arg(long)]
        parent_id: Option<String>,
    },
    /// Get items for a specific user (auth required)
    UserItems {
        user_id: String,
        #[arg(long)]
        parent_id: Option<String>,
        #[arg(long)]
        include_item_types: Option<String>,
        #[arg(long)]
        limit: Option<u32>,
        #[arg(long)]
        recursive: Option<bool>,
    },
    /// Get latest items for a specific user (auth required)
    UserItemsLatest {
        user_id: String,
        #[arg(long)]
        parent_id: Option<String>,
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Get resume items for a specific user (auth required)
    UserItemsResume {
        user_id: String,
    },

    // ── Images ───────────────────────────────────────────────
    /// List images for an item (auth required)
    ItemImages {
        id: String,
    },
    /// Get a specific image (auth required). Streams to stdout.
    ItemImage {
        id: String,
        image_type: String,
        #[arg(long)]
        max_width: Option<u32>,
        #[arg(long)]
        max_height: Option<u32>,
    },
    /// Upload an image for an item (auth required)
    ItemImageUpload {
        id: String,
        image_type: String,
        /// Path to the image file on disk
        file: String,
    },
    /// Refresh item metadata (auth required)
    ItemRefresh {
        id: String,
    },

    // ── QuickConnect ─────────────────────────────────────────
    /// Check if QuickConnect is enabled (public)
    QuickConnectEnabled,
    /// Initiate a QuickConnect session (public)
    QuickConnectInitiate,
    /// Poll QuickConnect status (public)
    QuickConnectConnect {
        secret: String,
    },
    /// Authorize a QuickConnect request (auth required)
    QuickConnectAuthorize {
        /// JSON body, e.g. '{"Secret":"..."}'
        json: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let client = Client::new();
    let base = cli.url.trim_end_matches('/');

    let result = run(&cli, &client, base).await;

    match result {
        Ok(output) => {
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Build a request with optional auth header.
fn auth_header(token: &Option<String>) -> Option<(&'static str, String)> {
    token.as_ref().map(|t| {
        ("Authorization", format!("MediaBrowser Token=\"{t}\""))
    })
}

fn apply_auth(
    mut req: reqwest::RequestBuilder,
    token: &Option<String>,
) -> reqwest::RequestBuilder {
    if let Some((k, v)) = auth_header(token) {
        req = req.header(k, v);
    }
    req
}

async fn run(
    cli: &Cli,
    client: &Client,
    base: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let token = &cli.token;

    match &cli.command {
        // ── System ───────────────────────────────────────────
        Commands::SystemInfoPublic => {
            Ok(get(client, &format!("{base}/System/Info/Public"), token).await?)
        }
        Commands::Ping => {
            let resp = client.get(format!("{base}/System/Ping")).send().await?;
            let text = resp.text().await?;
            Ok(Value::String(text))
        }
        Commands::Health => {
            let resp = client.get(format!("{base}/health")).send().await?;
            let text = resp.text().await?;
            Ok(Value::String(text))
        }
        Commands::SystemInfo => {
            Ok(get(client, &format!("{base}/System/Info"), token).await?)
        }
        Commands::SystemConfigGet => {
            Ok(get(client, &format!("{base}/System/Configuration"), token).await?)
        }
        Commands::SystemConfigUpdate { json } => {
            let body: Value = serde_json::from_str(json)?;
            Ok(post_json(client, &format!("{base}/System/Configuration"), token, &body).await?)
        }
        Commands::SystemShutdown => {
            Ok(post_empty(client, &format!("{base}/System/Shutdown"), token).await?)
        }
        Commands::SystemRestart => {
            Ok(post_empty(client, &format!("{base}/System/Restart"), token).await?)
        }
        Commands::SystemLogs => {
            Ok(get(client, &format!("{base}/System/Logs"), token).await?)
        }

        // ── Auth ─────────────────────────────────────────────
        Commands::Login { username, password } => {
            let body = serde_json::json!({
                "Username": username,
                "Pw": password,
            });
            Ok(post_json(client, &format!("{base}/Users/AuthenticateByName"), &None, &body).await?)
        }

        // ── Users ────────────────────────────────────────────
        Commands::UsersPublic => {
            Ok(get(client, &format!("{base}/Users/Public"), &None).await?)
        }
        Commands::Users => {
            Ok(get(client, &format!("{base}/Users"), token).await?)
        }
        Commands::UsersMe => {
            Ok(get(client, &format!("{base}/Users/Me"), token).await?)
        }
        Commands::UserGet { id } => {
            Ok(get(client, &format!("{base}/Users/{id}"), token).await?)
        }
        Commands::UserCreate { username, password } => {
            let body = serde_json::json!({
                "Name": username,
                "Password": password,
            });
            Ok(post_json(client, &format!("{base}/Users/New"), token, &body).await?)
        }
        Commands::UserDelete { id } => {
            Ok(post_empty(client, &format!("{base}/Users/{id}/Delete"), token).await?)
        }
        Commands::UserChangePassword { id, current_pw, new_pw } => {
            let body = serde_json::json!({
                "CurrentPw": current_pw,
                "NewPw": new_pw,
            });
            Ok(post_json(client, &format!("{base}/Users/{id}/Password"), token, &body).await?)
        }
        Commands::UserUpdatePolicy { id, json } => {
            let body: Value = serde_json::from_str(json)?;
            Ok(post_json(client, &format!("{base}/Users/{id}/Policy"), token, &body).await?)
        }
        Commands::UserUpdateConfig { id, json } => {
            let body: Value = serde_json::from_str(json)?;
            Ok(post_json(client, &format!("{base}/Users/{id}/Configuration"), token, &body).await?)
        }
        Commands::ForgotPassword { username } => {
            let body = serde_json::json!({ "EnteredUsername": username });
            Ok(post_json(client, &format!("{base}/Users/ForgotPassword"), &None, &body).await?)
        }

        // ── Library ──────────────────────────────────────────
        Commands::Libraries => {
            Ok(get(client, &format!("{base}/Library/VirtualFolders"), token).await?)
        }
        Commands::LibraryCreate { name, content_type, paths } => {
            let paths_vec: Vec<&str> = paths.split(',').map(|s| s.trim()).collect();
            let body = serde_json::json!({
                "Name": name,
                "ContentType": content_type,
                "Paths": paths_vec,
            });
            Ok(post_json(client, &format!("{base}/Library/VirtualFolders"), token, &body).await?)
        }
        Commands::LibraryRemove { id } => {
            Ok(delete(client, &format!("{base}/Library/VirtualFolders/{id}"), token).await?)
        }
        Commands::LibraryScan { id } => {
            Ok(post_empty(client, &format!("{base}/Library/VirtualFolders/{id}/Refresh"), token).await?)
        }
        Commands::LibraryRefreshAll => {
            Ok(post_empty(client, &format!("{base}/Library/Refresh"), token).await?)
        }

        // ── Items ────────────────────────────────────────────
        Commands::Items {
            parent_id, include_item_types, sort_by, sort_order,
            start_index, limit, recursive, search_term, genres, years,
        } => {
            let mut params = Vec::new();
            if let Some(v) = parent_id { params.push(("ParentId", v.clone())); }
            if let Some(v) = include_item_types { params.push(("IncludeItemTypes", v.clone())); }
            if let Some(v) = sort_by { params.push(("SortBy", v.clone())); }
            if let Some(v) = sort_order { params.push(("SortOrder", v.clone())); }
            if let Some(v) = start_index { params.push(("StartIndex", v.to_string())); }
            if let Some(v) = limit { params.push(("Limit", v.to_string())); }
            if let Some(v) = recursive { params.push(("Recursive", v.to_string())); }
            if let Some(v) = search_term { params.push(("SearchTerm", v.clone())); }
            if let Some(v) = genres { params.push(("Genres", v.clone())); }
            if let Some(v) = years { params.push(("Years", v.clone())); }
            Ok(get_query(client, &format!("{base}/Items"), token, &params).await?)
        }
        Commands::ItemGet { id } => {
            Ok(get(client, &format!("{base}/Items/{id}"), token).await?)
        }
        Commands::ItemDelete { id } => {
            Ok(delete(client, &format!("{base}/Items/{id}"), token).await?)
        }
        Commands::ItemChildren { id } => {
            Ok(get(client, &format!("{base}/Items/{id}/Children"), token).await?)
        }
        Commands::ItemSimilar { id, limit } => {
            let mut params = Vec::new();
            if let Some(v) = limit { params.push(("Limit", v.to_string())); }
            Ok(get_query(client, &format!("{base}/Items/{id}/Similar"), token, &params).await?)
        }
        Commands::ItemsLatest { parent_id, limit } => {
            let mut params = Vec::new();
            if let Some(v) = parent_id { params.push(("ParentId", v.clone())); }
            if let Some(v) = limit { params.push(("Limit", v.to_string())); }
            Ok(get_query(client, &format!("{base}/Items/Latest"), token, &params).await?)
        }
        Commands::ItemsResume => {
            Ok(get(client, &format!("{base}/Items/Resume"), token).await?)
        }
        Commands::ItemsFilters { parent_id } => {
            let mut params = Vec::new();
            if let Some(v) = parent_id { params.push(("ParentId", v.clone())); }
            Ok(get_query(client, &format!("{base}/Items/Filters"), token, &params).await?)
        }
        Commands::UserItems { user_id, parent_id, include_item_types, limit, recursive } => {
            let mut params = Vec::new();
            if let Some(v) = parent_id { params.push(("ParentId", v.clone())); }
            if let Some(v) = include_item_types { params.push(("IncludeItemTypes", v.clone())); }
            if let Some(v) = limit { params.push(("Limit", v.to_string())); }
            if let Some(v) = recursive { params.push(("Recursive", v.to_string())); }
            Ok(get_query(client, &format!("{base}/Users/{user_id}/Items"), token, &params).await?)
        }
        Commands::UserItemsLatest { user_id, parent_id, limit } => {
            let mut params = Vec::new();
            if let Some(v) = parent_id { params.push(("ParentId", v.clone())); }
            if let Some(v) = limit { params.push(("Limit", v.to_string())); }
            Ok(get_query(client, &format!("{base}/Users/{user_id}/Items/Latest"), token, &params).await?)
        }
        Commands::UserItemsResume { user_id } => {
            Ok(get(client, &format!("{base}/Users/{user_id}/Items/Resume"), token).await?)
        }

        // ── Images ───────────────────────────────────────────
        Commands::ItemImages { id } => {
            Ok(get(client, &format!("{base}/Items/{id}/Images"), token).await?)
        }
        Commands::ItemImage { id, image_type, max_width, max_height } => {
            let mut params = Vec::new();
            if let Some(v) = max_width { params.push(("maxWidth", v.to_string())); }
            if let Some(v) = max_height { params.push(("maxHeight", v.to_string())); }
            // Image endpoint returns binary; we report status + content-type
            let mut req = client.get(format!("{base}/Items/{id}/Images/{image_type}"));
            req = apply_auth(req, token);
            if !params.is_empty() {
                req = req.query(&params);
            }
            let resp = req.send().await?.error_for_status()?;
            let ct = resp.headers().get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("unknown")
                .to_string();
            let len = resp.content_length().unwrap_or(0);
            // Write binary to stdout
            let bytes = resp.bytes().await?;
            use std::io::Write;
            std::io::stdout().write_all(&bytes)?;
            // Return metadata to stderr so binary stdout stays clean
            eprintln!("Content-Type: {ct}, Size: {len} bytes");
            // Return a small JSON to satisfy the return type (won't be printed since
            // we already wrote binary to stdout – caller should redirect stdout to a file)
            Ok(serde_json::json!({
                "content_type": ct,
                "size": len,
            }))
        }
        Commands::ItemImageUpload { id, image_type, file } => {
            let bytes = tokio::fs::read(file).await?;
            let content_type = if file.ends_with(".png") {
                "image/png"
            } else if file.ends_with(".webp") {
                "image/webp"
            } else {
                "image/jpeg"
            };
            let mut req = client
                .post(format!("{base}/Items/{id}/Images/{image_type}"))
                .header("Content-Type", content_type)
                .body(bytes);
            req = apply_auth(req, token);
            let resp = req.send().await?.error_for_status()?;
            parse_response(resp).await
        }
        Commands::ItemRefresh { id } => {
            Ok(post_empty(client, &format!("{base}/Items/{id}/Refresh"), token).await?)
        }

        // ── QuickConnect ─────────────────────────────────────
        Commands::QuickConnectEnabled => {
            Ok(get(client, &format!("{base}/QuickConnect/Enabled"), &None).await?)
        }
        Commands::QuickConnectInitiate => {
            Ok(post_empty(client, &format!("{base}/QuickConnect/Initiate"), &None).await?)
        }
        Commands::QuickConnectConnect { secret } => {
            let params = vec![("secret", secret.clone())];
            Ok(get_query(client, &format!("{base}/QuickConnect/Connect"), &None, &params).await?)
        }
        Commands::QuickConnectAuthorize { json } => {
            let body: Value = serde_json::from_str(json)?;
            Ok(post_json(client, &format!("{base}/QuickConnect/Authorize"), token, &body).await?)
        }
    }
}

// ── HTTP helpers ─────────────────────────────────────────────

async fn get(
    client: &Client,
    url: &str,
    token: &Option<String>,
) -> Result<Value, Box<dyn std::error::Error>> {
    let req = apply_auth(client.get(url), token);
    let resp = req.send().await?.error_for_status()?;
    parse_response(resp).await
}

async fn get_query(
    client: &Client,
    url: &str,
    token: &Option<String>,
    params: &[(impl AsRef<str> + serde::Serialize, impl AsRef<str> + serde::Serialize)],
) -> Result<Value, Box<dyn std::error::Error>> {
    let req = apply_auth(client.get(url).query(params), token);
    let resp = req.send().await?.error_for_status()?;
    parse_response(resp).await
}

async fn post_json(
    client: &Client,
    url: &str,
    token: &Option<String>,
    body: &Value,
) -> Result<Value, Box<dyn std::error::Error>> {
    let req = apply_auth(client.post(url).json(body), token);
    let resp = req.send().await?.error_for_status()?;
    parse_response(resp).await
}

async fn post_empty(
    client: &Client,
    url: &str,
    token: &Option<String>,
) -> Result<Value, Box<dyn std::error::Error>> {
    let req = apply_auth(client.post(url), token);
    let resp = req.send().await?.error_for_status()?;
    parse_response(resp).await
}

async fn delete(
    client: &Client,
    url: &str,
    token: &Option<String>,
) -> Result<Value, Box<dyn std::error::Error>> {
    let req = apply_auth(client.delete(url), token);
    let resp = req.send().await?.error_for_status()?;
    parse_response(resp).await
}

async fn parse_response(
    resp: reqwest::Response,
) -> Result<Value, Box<dyn std::error::Error>> {
    let status = resp.status();
    let text = resp.text().await?;
    if text.is_empty() {
        Ok(serde_json::json!({ "status": status.as_u16() }))
    } else {
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }
}
