use super::types::*;

use super::types::{ModelDetail, ModelListResponse, RefreshResponse, VerifyResponse};

use crate::utils::{post_request, put_request};

pub async fn fetch_model(id: String) -> Option<ModelDetail> {
    if id == "new" {
        let resp = gloo_net::http::Request::get("/api/models")
            .send()
            .await
            .ok()?;
        let list: ModelListResponse = resp.json().await.ok()?;
        return Some(ModelDetail {
            id: 0,
            backend: list.backends.first().cloned().unwrap_or_default(),
            model: None,
            quant: None,
            args: vec![],
            sampling: None,
            enabled: true,
            context_length: None,
            num_parallel: Some(1),
            port: None,
            api_name: None,
            display_name: None,
            gpu_layers: None,
            quants: std::collections::BTreeMap::new(),
            backends: list.backends,
            mmproj: None,
            repo_commit_sha: None,
            repo_pulled_at: None,
            modalities: None,
        });
    }
    let encoded_id = urlencoding::encode(&id);
    let resp = gloo_net::http::Request::get(&format!("/api/models/{}", encoded_id))
        .send()
        .await;
    match resp {
        Ok(r) if r.status() == 200 => r.json::<ModelDetail>().await.ok(),
        _ => None,
    }
}

pub fn form_to_sampling_json(form: &ModelForm) -> serde_json::Value {
    let mut obj = serde_json::Map::new();

    if let Some(field) = form.sampling.get("temperature") {
        if field.enabled {
            if let Ok(val) = field.value.parse::<f64>() {
                obj.insert("temperature".to_string(), serde_json::json!(val));
            }
        }
    }
    if let Some(field) = form.sampling.get("top_k") {
        if field.enabled {
            if let Ok(val) = field.value.parse::<u64>() {
                obj.insert("top_k".to_string(), serde_json::json!(val));
            }
        }
    }
    if let Some(field) = form.sampling.get("top_p") {
        if field.enabled {
            if let Ok(val) = field.value.parse::<f64>() {
                obj.insert("top_p".to_string(), serde_json::json!(val));
            }
        }
    }
    if let Some(field) = form.sampling.get("min_p") {
        if field.enabled {
            if let Ok(val) = field.value.parse::<f64>() {
                obj.insert("min_p".to_string(), serde_json::json!(val));
            }
        }
    }
    if let Some(field) = form.sampling.get("presence_penalty") {
        if field.enabled {
            if let Ok(val) = field.value.parse::<f64>() {
                obj.insert("presence_penalty".to_string(), serde_json::json!(val));
            }
        }
    }
    if let Some(field) = form.sampling.get("frequency_penalty") {
        if field.enabled {
            if let Ok(val) = field.value.parse::<f64>() {
                obj.insert("frequency_penalty".to_string(), serde_json::json!(val));
            }
        }
    }
    if let Some(field) = form.sampling.get("repeat_penalty") {
        if field.enabled {
            if let Ok(val) = field.value.parse::<f64>() {
                obj.insert("repeat_penalty".to_string(), serde_json::json!(val));
            }
        }
    }

    if obj.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::json!(obj)
    }
}

pub async fn save_model(args: Vec<String>, form: ModelForm, is_new: bool) -> Result<(), String> {
    let sampling = form_to_sampling_json(&form);

    let body = serde_json::json!({
        "id": form.id,
        "backend": form.backend,
        "model": form.model,
        "quant": form.quant,
        "mmproj": form.mmproj,
        "args": args,
        "sampling": sampling,
        "enabled": form.enabled,
        "context_length": form.context_length,
        "num_parallel": form.num_parallel,
        "port": form.port,
        "api_name": form.api_name,
        "display_name": form.display_name,
        "gpu_layers": form.gpu_layers,
        "quants": form.quants,
        "modalities": form.modalities,
    });

    let encoded_id = urlencoding::encode(&form.id);
    let (url, is_post) = if is_new {
        ("/api/models".to_string(), true)
    } else {
        (format!("/api/models/{}", encoded_id), false)
    };

    let req = if is_post {
        post_request(&url)
    } else {
        put_request(&url)
    };

    let resp = req
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if resp.status() == 200 || resp.status() == 201 {
        Ok(())
    } else {
        let text = resp.text().await.unwrap_or_else(|_| "Unknown error".into());
        Err(text)
    }
}

pub async fn rename_model(old_id: &str, new_id: &str) -> Result<(), String> {
    let body = serde_json::json!({ "new_id": new_id });
    let encoded_id = urlencoding::encode(old_id);
    let resp = post_request(&format!("/api/models/{}/rename", encoded_id))
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if resp.status() == 200 {
        Ok(())
    } else {
        let text = resp.text().await.unwrap_or_else(|_| "Unknown error".into());
        Err(text)
    }
}

pub async fn delete_model_api(id: String) -> Result<(), String> {
    let encoded_id = urlencoding::encode(&id);
    let resp = gloo_net::http::Request::delete(&format!("/api/models/{}", encoded_id))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status() == 200 {
        Ok(())
    } else {
        let text = resp.text().await.unwrap_or_else(|_| "Unknown error".into());
        Err(text)
    }
}

pub async fn delete_quant_api(id: String, quant_key: String) -> Result<(), String> {
    let encoded_id = urlencoding::encode(&id);
    let encoded_key = urlencoding::encode(&quant_key);
    let resp = gloo_net::http::Request::delete(&format!(
        "/api/models/{}/quants/{}",
        encoded_id, encoded_key
    ))
    .send()
    .await
    .map_err(|e| e.to_string())?;
    if resp.status() == 200 {
        Ok(())
    } else {
        let text = resp.text().await.unwrap_or_else(|_| "Unknown error".into());
        Err(text)
    }
}

pub async fn refresh_model_api(id: String) -> Result<RefreshResponse, String> {
    // Percent-encode the id for safe path interpolation; model ids may
    // contain `/`, spaces, or other reserved characters.
    let encoded_id = urlencoding::encode(&id);
    let resp = post_request(&format!("/api/models/{}/refresh", encoded_id))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status() != 200 {
        let text = resp.text().await.unwrap_or_else(|_| "Unknown error".into());
        return Err(text);
    }
    resp.json::<RefreshResponse>()
        .await
        .map_err(|e| format!("Failed to parse refresh response: {}", e))
}

pub async fn verify_model_api(id: String) -> Result<VerifyResponse, String> {
    let encoded_id = urlencoding::encode(&id);
    let resp = post_request(&format!("/api/models/{}/verify", encoded_id))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status() != 200 {
        let text = resp.text().await.unwrap_or_else(|_| "Unknown error".into());
        return Err(text);
    }
    resp.json::<VerifyResponse>()
        .await
        .map_err(|e| format!("Failed to parse verify response: {}", e))
}

pub async fn fetch_sampling_templates(
) -> Option<std::collections::HashMap<String, serde_json::Value>> {
    let resp = gloo_net::http::Request::get("/api/models")
        .send()
        .await
        .ok()?;
    let list: ModelListResponse = resp.json().await.ok()?;
    let templates = list.sampling_templates?;
    Some(templates)
}
