use std::collections::HashMap;

use actix_web::{HttpResponse, Responder, http, web};
use log::{debug, error, trace, warn};

use super::super::state::ServerState;
use super::releases::serve_release_file;

pub async fn proxy_api_request(
    path: web::Path<String>,
    query: web::Query<HashMap<String, String>>,
    state: web::Data<ServerState>,
) -> impl Responder {
    let path_str = path.into_inner();

    debug!("API request for path: {}", path_str);
    for (key, value) in query.iter() {
        trace!("  Query param: {}={}", key, value);
    }

    if !state.config.proxy_mode {
        warn!("Rejecting proxy request in local mode: {}", path_str);
        return HttpResponse::NotFound().body(format!("API path not found locally: {}", path_str));
    }

    if path_str.starts_with("api/releases/stable/") || path_str.starts_with("releases/stable/") {
        let clean_path = path_str.trim_start_matches("api/");

        let parts: Vec<&str> = clean_path.split('/').collect();
        if parts.len() >= 4 {
            let version = parts[2];
            let filename = parts[3];

            if let Some(releases_dir) = &state.config.releases_dir {
                let zed_path = releases_dir.join("zed").join(format!(
                    "zed-{}-{}.gz",
                    version,
                    filename.replace(".tar.gz", "")
                ));
                if zed_path.exists() {
                    return serve_release_file(&zed_path);
                }

                let remote_server_path = releases_dir.join("zed-remote-server").join(format!(
                    "zed-remote-server-{}-{}.gz",
                    version,
                    filename.replace(".tar.gz", "")
                ));
                if remote_server_path.exists() {
                    return serve_release_file(&remote_server_path);
                }
            }
        }
    }

    if state.config.releases_dir.is_some()
        && path_str.starts_with("releases/")
        && path_str != "releases/latest"
    {
        let clean_path = path_str.split('?').next().unwrap_or(&path_str);
        let file_path = state
            .config
            .releases_dir
            .as_ref()
            .unwrap()
            .join(clean_path.trim_start_matches("releases/"));
        debug!("Attempting to serve release file from: {:?}", file_path);

        if file_path.exists() {
            return serve_release_file(&file_path);
        } else {
            debug!("Release file not found locally: {:?}", file_path);
        }
    }

    let client = match reqwest::Client::builder().user_agent("zedex").build() {
        Ok(client) => client,
        Err(e) => {
            error!("Error creating HTTP client: {}", e);
            return HttpResponse::InternalServerError()
                .body(format!("Error creating HTTP client: {}", e));
        }
    };
    let mut url = format!("https://zed.dev/api/{}", path_str);

    if !query.is_empty() {
        url.push('?');
        let query_string = query
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("&");
        url.push_str(&query_string);
    }

    debug!("Proxying request to: {}", url);

    match client.get(&url).send().await {
        Ok(response) => {
            let status = response.status();
            debug!("Proxy response status: {}", status);

            let content_type = response
                .headers()
                .get(http::header::CONTENT_TYPE)
                .and_then(|h| h.to_str().ok())
                .unwrap_or("application/json")
                .to_string();

            let body = response.bytes().await.unwrap_or_default();

            debug!("Response content type: {}", content_type);
            debug!("Response size: {} bytes", body.len());

            HttpResponse::build(
                http::StatusCode::from_u16(status.as_u16()).unwrap_or(http::StatusCode::OK),
            )
            .content_type(content_type)
            .body(body)
        }
        Err(e) => {
            error!("Error proxying request: {}", e);
            HttpResponse::InternalServerError().body(format!("Error proxying request: {}", e))
        }
    }
}

pub async fn proxy_extensions_updates(query: web::Query<HashMap<String, String>>) -> HttpResponse {
    debug!("Proxying extension updates request to api.zed.dev");

    let client = match reqwest::Client::builder().user_agent("zedex").build() {
        Ok(client) => client,
        Err(e) => {
            error!("Error creating HTTP client: {}", e);
            return HttpResponse::InternalServerError()
                .body(format!("Error creating HTTP client: {}", e));
        }
    };

    let mut url = "https://api.zed.dev/extensions/updates".to_string();

    if !query.is_empty() {
        url.push('?');
        let query_string = query
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("&");
        url.push_str(&query_string);
    }

    debug!("Proxying extension updates to: {}", url);

    match client.get(&url).send().await {
        Ok(response) => match response.error_for_status() {
            Ok(response) => match response.bytes().await {
                Ok(bytes) => HttpResponse::Ok()
                    .content_type("application/json")
                    .body(bytes),
                Err(e) => {
                    error!("Error reading proxied response: {}", e);
                    HttpResponse::InternalServerError()
                        .body(format!("Error reading proxied response: {}", e))
                }
            },
            Err(e) => {
                error!("Error from proxied server: {}", e);
                match e.status() {
                    Some(status) => HttpResponse::build(
                        http::StatusCode::from_u16(status.as_u16())
                            .unwrap_or(http::StatusCode::INTERNAL_SERVER_ERROR),
                    )
                    .body(format!("Error from zed.dev: {}", e)),
                    None => HttpResponse::InternalServerError()
                        .body(format!("Error from zed.dev: {}", e)),
                }
            }
        },
        Err(e) => {
            error!("Error proxying request: {}", e);
            HttpResponse::InternalServerError().body(format!("Error proxying request: {}", e))
        }
    }
}

pub async fn proxy_extension_versions(extension_id: String) -> HttpResponse {
    let url = format!("https://api.zed.dev/extensions/{}", extension_id);
    debug!("Proxying extension versions request to: {}", url);

    let client = reqwest::Client::new();
    match client.get(&url).send().await {
        Ok(resp) => {
            let status = resp.status();
            let headers = resp.headers().clone();

            match resp.bytes().await {
                Ok(bytes) => {
                    let mut builder = HttpResponse::build(status);

                    for (key, value) in headers.iter() {
                        if let Ok(header_value) =
                            http::header::HeaderValue::from_bytes(value.as_bytes())
                        {
                            builder.append_header((key.clone(), header_value));
                        }
                    }

                    builder.body(bytes)
                }
                Err(e) => {
                    error!("Failed to get response body from proxy request: {}", e);
                    HttpResponse::InternalServerError().body(format!("Proxy error: {}", e))
                }
            }
        }
        Err(e) => {
            error!("Failed to proxy extension versions request: {}", e);
            HttpResponse::InternalServerError().body(format!("Proxy error: {}", e))
        }
    }
}

pub async fn proxy_download_request(extension_id: String) -> HttpResponse {
    let url = format!(
        "https://api.zed.dev/extensions/{}/download?min_schema_version=0&max_schema_version=100&min_wasm_api_version=0.0.0&max_wasm_api_version=100.0.0",
        extension_id
    );
    debug!("Proxying extension download request to: {}", url);

    let client = reqwest::Client::new();
    match client.get(&url).send().await {
        Ok(resp) => {
            let status = resp.status();
            let headers = resp.headers().clone();

            match resp.bytes().await {
                Ok(bytes) => {
                    let mut builder = HttpResponse::build(status);

                    for (key, value) in headers.iter() {
                        if let Ok(header_value) =
                            http::header::HeaderValue::from_bytes(value.as_bytes())
                        {
                            builder.append_header((key.clone(), header_value));
                        }
                    }

                    builder.body(bytes)
                }
                Err(e) => {
                    error!("Failed to get response body from proxy request: {}", e);
                    HttpResponse::InternalServerError().body(format!("Proxy error: {}", e))
                }
            }
        }
        Err(e) => {
            error!("Failed to proxy extension download request: {}", e);
            HttpResponse::InternalServerError().body(format!("Proxy error: {}", e))
        }
    }
}

pub async fn proxy_download_version_request(extension_id: String, version: String) -> HttpResponse {
    let url = format!(
        "https://api.zed.dev/extensions/{}/{}/download",
        extension_id, version
    );
    debug!("Proxying versioned extension download request to: {}", url);

    let client = reqwest::Client::new();
    match client.get(&url).send().await {
        Ok(resp) => {
            let status = resp.status();
            let headers = resp.headers().clone();

            match resp.bytes().await {
                Ok(bytes) => {
                    let mut builder = HttpResponse::build(status);

                    for (key, value) in headers.iter() {
                        if let Ok(header_value) =
                            http::header::HeaderValue::from_bytes(value.as_bytes())
                        {
                            builder.append_header((key.clone(), header_value));
                        }
                    }

                    builder.body(bytes)
                }
                Err(e) => {
                    error!("Failed to get response body from proxy request: {}", e);
                    HttpResponse::InternalServerError().body(format!("Proxy error: {}", e))
                }
            }
        }
        Err(e) => {
            error!("Failed to proxy extension version download request: {}", e);
            HttpResponse::InternalServerError().body(format!("Proxy error: {}", e))
        }
    }
}

pub async fn proxy_version_request(os: String, arch: String, asset: String) -> HttpResponse {
    debug!(
        "Proxying version request for {}-{}-{} to zed.dev",
        asset, os, arch
    );

    let client = reqwest::Client::new();
    let url = format!(
        "https://zed.dev/api/releases/latest?asset={}&os={}&arch={}",
        asset, os, arch
    );

    match client.get(&url).send().await {
        Ok(response) => match response.error_for_status() {
            Ok(response) => match response.bytes().await {
                Ok(bytes) => HttpResponse::Ok()
                    .content_type("application/json")
                    .body(bytes),
                Err(e) => {
                    error!("Error reading proxied response: {}", e);
                    HttpResponse::InternalServerError()
                        .body(format!("Error reading proxied response: {}", e))
                }
            },
            Err(e) => {
                error!("Error from proxied server: {}", e);
                match e.status() {
                    Some(status) => HttpResponse::build(
                        http::StatusCode::from_u16(status.as_u16())
                            .unwrap_or(http::StatusCode::INTERNAL_SERVER_ERROR),
                    )
                    .body(format!("Error from zed.dev: {}", e)),
                    None => HttpResponse::InternalServerError()
                        .body(format!("Error from zed.dev: {}", e)),
                }
            }
        },
        Err(e) => {
            error!("Error proxying request: {}", e);
            HttpResponse::InternalServerError().body(format!("Error proxying request: {}", e))
        }
    }
}
