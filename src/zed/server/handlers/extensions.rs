use std::{collections::HashMap, fs};

use actix_web::{web, HttpResponse, Responder};
use log::{debug, error, info, warn};
use semver::Version as SemverVersion;

use crate::zed::{extensions_utils, WrappedExtensions};

use super::super::state::ServerState;
use super::proxy::{
    proxy_download_request, proxy_download_version_request, proxy_extension_versions,
    proxy_extensions_updates,
};

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(web::resource("/extensions").to(get_extensions_index))
        .service(web::resource("/extensions/updates").to(check_extension_updates))
        .service(web::resource("/extensions/{id}/download").to(download_extension))
        .service(
            web::resource("/extensions/{id}/{version}/download")
                .to(download_extension_with_version),
        )
        .service(web::resource("/extensions/{id}").to(get_extension_versions));
}

fn filter_extensions_with_params(
    extensions: &WrappedExtensions,
    filter: Option<&str>,
    min_schema_version: Option<i32>,
    max_schema_version: Option<i32>,
    min_wasm_api_version: Option<&str>,
    max_wasm_api_version: Option<&str>,
    provides: Option<&str>,
    extension_ids: Option<&[&str]>,
) -> crate::zed::Extensions {
    let filtered_by_standard = extensions_utils::filter_extensions(
        &extensions.data,
        filter,
        max_schema_version,
        provides,
    );

    let filtered_by_min_schema = if let Some(min_version) = min_schema_version {
        filtered_by_standard
            .into_iter()
            .filter(|ext| ext.schema_version >= min_version)
            .collect()
    } else {
        filtered_by_standard
    };

    let filtered_by_ids = if let Some(ids) = extension_ids {
        if !ids.is_empty() {
            filtered_by_min_schema
                .into_iter()
                .filter(|ext| ids.contains(&ext.id.as_str()))
                .collect()
        } else {
            filtered_by_min_schema
        }
    } else {
        filtered_by_min_schema
    };

    if min_wasm_api_version.is_some() || max_wasm_api_version.is_some() {
        filtered_by_ids
            .into_iter()
            .filter(|ext| {
                if ext.wasm_api_version.is_none() {
                    return true;
                }

                let ext_version = ext.wasm_api_version.as_ref().unwrap();

                if let Some(min_version) = min_wasm_api_version {
                    if ext_version.as_str() < min_version {
                        return false;
                    }
                }

                if let Some(max_version) = max_wasm_api_version {
                    if ext_version.as_str() > max_version {
                        return false;
                    }
                }

                true
            })
            .collect()
    } else {
        filtered_by_ids
    }
}

pub async fn get_extensions_index(
    state: web::Data<ServerState>,
    query: web::Query<HashMap<String, String>>,
) -> impl Responder {
    let extensions_file = state.config.extensions_dir.join("extensions.json");

    match fs::read_to_string(&extensions_file) {
        Ok(content) => match serde_json::from_str::<WrappedExtensions>(&content) {
            Ok(extensions) => {
                let filter = query.get("filter").map(|s| s.as_str());
                let max_schema_version = query
                    .get("max_schema_version")
                    .and_then(|v| v.parse::<i32>().ok());
                let provides = query.get("provides").map(|s| s.as_str());

                debug!(
                    "Filtering extensions: filter={:?}, max_schema_version={:?}, provides={:?}",
                    filter, max_schema_version, provides
                );

                let filtered_extensions = filter_extensions_with_params(
                    &extensions,
                    filter,
                    None,
                    max_schema_version,
                    None,
                    None,
                    provides,
                    None,
                );

                info!(
                    "Serving {} filtered extensions from index",
                    filtered_extensions.len()
                );

                let wrapped = WrappedExtensions {
                    data: filtered_extensions,
                };
                HttpResponse::Ok().json(wrapped)
            }
            Err(e) => {
                error!("Error parsing extensions.json: {}", e);
                HttpResponse::InternalServerError()
                    .body(format!("Error parsing extensions file: {}", e))
            }
        },
        Err(e) => {
            error!("Error reading extensions.json: {}", e);
            HttpResponse::NotFound().body(format!("Extensions file not found: {}", e))
        }
    }
}

pub async fn download_extension(
    path: web::Path<String>,
    state: web::Data<ServerState>,
) -> impl Responder {
    let id = path.into_inner();
    let ext_dir = state.config.extensions_dir.join(&id);

    let latest_file_path = ext_dir.join(format!("{}.tgz", id));
    debug!("Checking for latest version: {}", latest_file_path.display());

    if let Ok(bytes) = fs::read(&latest_file_path) {
        info!("Serving latest version for {}", id);
        return HttpResponse::Ok()
            .content_type("application/gzip")
            .body(bytes);
    }

    if ext_dir.exists() {
        let versions_file = ext_dir.join("versions.json");

        if versions_file.exists() {
            debug!("Looking for highest available version in {}", versions_file.display());

            if let Ok(content) = fs::read_to_string(&versions_file) {
                if let Ok(versions) = serde_json::from_str::<WrappedExtensions>(&content) {
                    let highest_version = versions
                        .data
                        .iter()
                        .filter_map(|ext| {
                            let version = &ext.version;
                            let archive_path = ext_dir.join(format!("{}-{}.tgz", id, version));

                            if archive_path.exists() {
                                SemverVersion::parse(version)
                                    .map(|v| (v, version.clone(), archive_path))
                                    .or_else(|e| {
                                        warn!("Invalid version '{}' for {}: {}", version, id, e);
                                        Err(e)
                                    })
                                    .ok()
                            } else {
                                None
                            }
                        })
                        .max_by(|(v1, _, _), (v2, _, _)| v1.cmp(v2));

                    if let Some((_, version_str, file_path)) = highest_version {
                        info!(
                            "Serving highest downloaded version {} for {}",
                            version_str, id
                        );

                        if let Ok(bytes) = fs::read(&file_path) {
                            return HttpResponse::Ok()
                                .content_type("application/gzip")
                                .body(bytes);
                        } else {
                            error!("Failed to read archive file: {}", file_path.display());
                        }
                    } else {
                        debug!("No downloaded versions found for {}", id);
                    }
                } else {
                    error!("Failed to parse versions.json for {}", id);
                }
            } else {
                error!("Failed to read versions.json for {}", id);
            }
        }
    }

    let old_path = state
        .config
        .extensions_dir
        .join(format!("{}.tar.gz", id));
    debug!("Checking old structure: {}", old_path.display());

    if let Ok(bytes) = fs::read(&old_path) {
        info!("Serving extension from old structure for {}", id);
        return HttpResponse::Ok()
            .content_type("application/gzip")
            .body(bytes);
    }

    if state.config.proxy_mode {
        error!("Extension not found locally for {}, proxying request", id);
        proxy_download_request(id).await
    } else {
        error!(
            "Extension not found locally for {} and proxy mode is off",
            id
        );
        HttpResponse::NotFound().body(format!("Extension archive not found for id: {}", id))
    }
}

pub async fn download_extension_with_version(
    path: web::Path<(String, String)>,
    state: web::Data<ServerState>,
) -> impl Responder {
    let (id, version) = path.into_inner();
    debug!("Requested extension {} with version {}", id, version);

    let ext_dir = state.config.extensions_dir.join(&id);
    let versioned_file_path = ext_dir.join(format!("{}-{}.tgz", id, version));

    debug!("Looking for versioned extension at {:?}", versioned_file_path);
    match fs::read(&versioned_file_path) {
        Ok(bytes) => {
            info!("Successfully served extension archive: {} version {}", id, version);
            HttpResponse::Ok()
                .content_type("application/gzip")
                .body(bytes)
        }
        Err(_) => {
            if state.config.proxy_mode {
                error!(
                    "Extension version file not found, proxying: {} version {}",
                    id, version
                );
                proxy_download_version_request(id, version).await
            } else {
                error!("Extension version file not found: {} version {}", id, version);
                HttpResponse::NotFound()
                    .body(format!("Extension version archive not found: {}", version))
            }
        }
    }
}

pub async fn get_extension_versions(
    path: web::Path<String>,
    state: web::Data<ServerState>,
) -> impl Responder {
    let id = path.into_inner();
    let ext_dir = state.config.extensions_dir.join(&id);
    let versions_file = ext_dir.join("versions.json");

    debug!("Attempting to serve versions for extension id: {}", id);

    if versions_file.exists() {
        match fs::read_to_string(&versions_file) {
            Ok(content) => match serde_json::from_str::<WrappedExtensions>(&content) {
                Ok(extensions) => {
                    info!(
                        "Successfully served {} versions for extension: {}",
                        extensions.data.len(),
                        id
                    );
                    HttpResponse::Ok().json(extensions)
                }
                Err(e) => {
                    error!("Error parsing versions.json for {}: {}", id, e);
                    HttpResponse::InternalServerError()
                        .body(format!("Error parsing versions file: {}", e))
                }
            },
            Err(e) => {
                error!("Error reading versions.json for {}: {}", id, e);
                HttpResponse::InternalServerError()
                    .body(format!("Error reading versions file: {}", e))
            }
        }
    } else if state.config.proxy_mode {
        info!(
            "Extension versions file not found for {}. Proxying request in proxy mode.",
            id
        );
        proxy_extension_versions(id).await
    } else {
        error!(
            "Extension versions file not found for {}: {:?}",
            id, versions_file
        );
        HttpResponse::NotFound().body(format!("Extension versions not found for: {}", id))
    }
}

pub async fn check_extension_updates(
    state: web::Data<ServerState>,
    query: web::Query<HashMap<String, String>>,
) -> impl Responder {
    let min_schema_version = query
        .get("min_schema_version")
        .and_then(|v| v.parse::<i32>().ok());
    let max_schema_version = query
        .get("max_schema_version")
        .and_then(|v| v.parse::<i32>().ok());
    let min_wasm_api_version = query.get("min_wasm_api_version").map(|s| s.as_str());
    let max_wasm_api_version = query.get("max_wasm_api_version").map(|s| s.as_str());
    let ids_param = query.get("ids").cloned().unwrap_or_default();

    let extension_ids: Vec<&str> = if !ids_param.is_empty() {
        ids_param.split(',').collect()
    } else {
        Vec::new()
    };

    if ids_param.is_empty() {
        info!("No extensions to check for updates (empty ids parameter)");
        return HttpResponse::Ok().json(WrappedExtensions { data: Vec::new() });
    }

    debug!(
        "Extension update check: min_schema={:?}, max_schema={:?}, min_wasm_api={:?}, max_wasm_api={:?}, ids={:?}",
        min_schema_version, max_schema_version, min_wasm_api_version, max_wasm_api_version, extension_ids
    );

    let extensions_file = state.config.extensions_dir.join("extensions.json");

    match fs::read_to_string(&extensions_file) {
        Ok(content) => match serde_json::from_str::<WrappedExtensions>(&content) {
            Ok(extensions) => {
                let filtered_extensions = filter_extensions_with_params(
                    &extensions,
                    None,
                    min_schema_version,
                    max_schema_version,
                    min_wasm_api_version,
                    max_wasm_api_version,
                    None,
                    if extension_ids.is_empty() {
                        None
                    } else {
                        Some(&extension_ids)
                    },
                );

                info!(
                    "Serving {} updated extensions from index",
                    filtered_extensions.len()
                );

                let wrapped = WrappedExtensions {
                    data: filtered_extensions,
                };
                HttpResponse::Ok().json(wrapped)
            }
            Err(e) => {
                error!("Error parsing extensions.json: {}", e);
                HttpResponse::InternalServerError()
                    .body(format!("Error parsing extensions file: {}", e))
            }
        },
        Err(e) => {
            error!("Error reading extensions.json: {}", e);

            if state.config.proxy_mode {
                return proxy_extensions_updates(query).await;
            }

            HttpResponse::NotFound().body(format!("Extensions file not found: {}", e))
        }
    }
}
