//! Unit tests for `ProviderStore`, split out of `store.rs` so the production
//! file stays within the AGENTS §4 file-size limit. Declared as a descendant
//! of `store.rs`'s `tests` module via `#[path]`; the suite imports exactly
//! what it needs, so the two `use` lists stay explicit.

use crate::service::store::ProviderStore;
use ark_contract::{
    ProviderEntry, ProviderModelCatalogRequest, ProviderModelInventoryItem, ProviderTestRequest,
};
use std::path::{Path, PathBuf};

fn temp_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ark-provider-store-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn entry(id: &str, key: Option<&str>) -> ProviderEntry {
    let mut entry = ProviderEntry::new(id, "deepseek-chat");
    entry.base_url = "https://api.deepseek.com".into();
    entry.api_key = key.map(str::to_string);
    entry.endpoint_format = "openai_chat".into();
    entry
}

fn store(dir: &Path) -> ProviderStore {
    ProviderStore::new(dir.to_path_buf())
}

#[tokio::test]
async fn model_catalog_rejects_an_unknown_stored_provider_without_network_access() {
    let dir = temp_dir("catalog-missing");
    let error = store(&dir)
        .models(ProviderModelCatalogRequest {
            provider_id: Some("missing".into()),
            connection: None,
        })
        .await
        .unwrap_err();

    assert_eq!(error.code, ark_common::ArkErrorCode::NotFound);
    assert!(
        error.message.contains("does not exist"),
        "{}",
        error.message
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn model_catalog_uses_the_saved_auth_and_custom_headers() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4_096];
        let count = socket.read(&mut request).unwrap();
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 102\r\nConnection: close\r\n\r\n{\"data\":[{\"id\":\"vision\",\"context_length\":65536,\"architecture\":{\"input_modalities\":[\"text\",\"image\"]}}]}",
            )
            .unwrap();
        String::from_utf8_lossy(&request[..count]).into_owned()
    });
    let dir = temp_dir("catalog-http");
    let store = store(&dir);
    let mut configured = entry("p", Some("secret-catalog-key"));
    configured.base_url = format!("http://{address}/v1");
    configured.headers.insert("X-Tenant".into(), "ark".into());
    store.upsert(configured, false).unwrap();

    let catalog = store
        .models(ProviderModelCatalogRequest {
            provider_id: Some("p".into()),
            connection: None,
        })
        .await
        .unwrap();

    assert_eq!(catalog.models[0].id, "vision");
    assert_eq!(catalog.models[0].context_window_tokens, Some(65_536));
    assert_eq!(catalog.models[0].supports_vision, Some(true));
    let seen = server.join().unwrap().to_ascii_lowercase();
    assert!(seen.starts_with("get /v1/models"), "{seen}");
    assert!(
        seen.contains("authorization: bearer secret-catalog-key"),
        "{seen}"
    );
    assert!(seen.contains("x-tenant: ark"), "{seen}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_disabled_provider_can_still_be_discovered_before_it_is_enabled() {
    let dir = temp_dir("catalog-disabled");
    let store = store(&dir);
    let mut configured = entry("p", Some("secret-catalog-key"));
    configured.enabled = false;
    store.upsert(configured, false).unwrap();

    let config = store
        .config_for_catalog(ProviderModelCatalogRequest {
            provider_id: Some("p".into()),
            connection: None,
        })
        .unwrap();

    assert_eq!(config.provider_id, "p");
    assert_eq!(config.api_key, "secret-catalog-key");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_blank_id_is_a_validation_error() {
    let dir = temp_dir("blank-id");
    let err = store(&dir)
        .upsert(entry("  ", Some("sk-1234567890")), false)
        .unwrap_err();
    assert_eq!(err.code, ark_common::ArkErrorCode::ValidationError);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_invalid_endpoint_format_is_a_validation_error() {
    let dir = temp_dir("bad-format");
    let mut bad = entry("p", Some("sk-1234567890"));
    bad.endpoint_format = "grpc".into();
    let err = store(&dir).upsert(bad, false).unwrap_err();
    assert_eq!(err.code, ark_common::ArkErrorCode::ValidationError);
    assert!(err.message.contains("openai_chat"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_out_of_range_temperature_is_rejected() {
    let dir = temp_dir("temp");
    let mut hot = entry("p", Some("sk-1234567890"));
    hot.temperature = Some(2.5);
    let err = store(&dir).upsert(hot, false).unwrap_err();
    assert!(err.message.contains("temperature must be between 0 and 2"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn list_never_returns_the_plaintext_key() {
    let dir = temp_dir("mask");
    let store = store(&dir);
    store
        .upsert(entry("p", Some("sk-1234567890abcdef")), false)
        .unwrap();
    let list = store.list();
    assert_eq!(list.providers.len(), 1);
    assert!(list.providers[0].has_api_key);
    assert_eq!(list.providers[0].api_key_hint.as_deref(), Some("sk-1***ef"));
    let json = serde_json::to_string(&list).unwrap();
    assert!(!json.contains("567890"), "{json}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn list_reports_the_window_and_the_threshold_it_derives() {
    // The renderer divides its meter by the window and draws the threshold
    // inside it, so both numbers must describe the same resolved provider.
    let dir = temp_dir("window");
    let store = store(&dir);
    // A resolved provider needs a key: config_from_entry drops keyless
    // entries, so `list()` would report `None` for both numbers.
    let mut sized = entry("p", Some("sk-window-1234567890"));
    sized.context_window_tokens = Some(200_000);
    sized.max_tokens = Some(20_000);
    store.upsert(sized, true).unwrap();

    let list = store.list();
    assert_eq!(list.context_window_tokens, Some(200_000));
    // 80% of the usable window (200K minus the 20K output reserve).
    assert_eq!(list.compaction_threshold_tokens, Some(144_000));

    // A provider that publishes no window still compacts against the
    // default, and the meter divides by that same fallback.
    let mut unwindowed = entry("q", Some("sk-window-0987654321"));
    unwindowed.max_tokens = Some(20_000);
    store.upsert(unwindowed, true).unwrap();
    let list = store.list();
    assert_eq!(list.context_window_tokens, Some(256_000));
    assert_eq!(list.compaction_threshold_tokens, Some(188_800));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn edit_read_returns_the_stored_key_while_list_stays_redacted() {
    let dir = temp_dir("edit-read");
    let store = store(&dir);
    store
        .upsert(entry("p", Some("sk-1234567890abcdef")), false)
        .unwrap();
    let full = store.get("p").unwrap();
    assert_eq!(full.api_key.as_deref(), Some("sk-1234567890abcdef"));
    let list = store.list();
    let json = serde_json::to_string(&list).unwrap();
    assert!(!json.contains("567890"), "{json}");
    assert!(store.get("missing").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_short_key_is_masked_entirely() {
    let dir = temp_dir("short-key");
    let store = store(&dir);
    store.upsert(entry("p", Some("sk-123")), false).unwrap();
    let list = store.list();
    assert_eq!(list.providers[0].api_key_hint.as_deref(), Some("***"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_upsert_without_a_key_keeps_the_stored_one() {
    let dir = temp_dir("keep-key");
    let store = store(&dir);
    store
        .upsert(entry("p", Some("sk-1234567890abcdef")), false)
        .unwrap();
    let mut edit = entry("p", None);
    edit.model = "deepseek-reasoner".into();
    let list = store.upsert(edit, false).unwrap();
    assert!(list.providers[0].has_api_key);
    assert_eq!(
        store.resolve(Some("p")).unwrap().api_key,
        "sk-1234567890abcdef"
    );
    assert_eq!(store.resolve(Some("p")).unwrap().model, "deepseek-reasoner");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn gui_edit_without_advanced_fields_keeps_the_stored_advanced_values() {
    let dir = temp_dir("keep-advanced");
    let store = store(&dir);
    let mut saved = entry("p", Some("sk-1234567890abcdef"));
    saved.headers.insert("X-Trace".into(), "enabled".into());
    saved.max_tokens = Some(4_096);
    saved.temperature = Some(0.3);
    store.upsert(saved, false).unwrap();

    let mut edit = entry("p", None);
    edit.model = "deepseek-reasoner".into();
    store.upsert(edit, false).unwrap();

    let retained = store.lock().get("p").cloned().unwrap();
    assert_eq!(
        retained.headers.get("X-Trace"),
        Some(&"enabled".to_string())
    );
    assert_eq!(retained.max_tokens, Some(4_096));
    assert_eq!(retained.temperature, Some(0.3));
    assert_eq!(retained.model, "deepseek-reasoner");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn deleting_the_default_clears_it_without_panicking() {
    let dir = temp_dir("delete-default");
    let store = store(&dir);
    store
        .upsert(entry("p", Some("sk-1234567890")), true)
        .unwrap();
    assert_eq!(store.list().default_provider_id.as_deref(), Some("p"));
    let list = store.delete("p").unwrap();
    assert_eq!(list.default_provider_id, None);
    assert!(list.providers.is_empty());
    assert!(store.resolve(None).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn deleting_an_unknown_provider_is_not_found() {
    let dir = temp_dir("delete-missing");
    let err = store(&dir).delete("ghost").unwrap_err();
    assert_eq!(err.code, ark_common::ArkErrorCode::NotFound);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_default_must_exist_and_be_enabled() {
    let dir = temp_dir("set-default");
    let store = store(&dir);
    assert!(store.set_default("ghost", None).is_err());
    let mut off = entry("off", Some("sk-1234567890"));
    off.enabled = false;
    store.upsert(off, false).unwrap();
    let err = store.set_default("off", None).unwrap_err();
    assert!(err.message.contains("disabled"), "{}", err.message);
    store
        .upsert(entry("on", Some("sk-1234567890")), false)
        .unwrap();
    assert_eq!(
        store
            .set_default("on", None)
            .unwrap()
            .default_provider_id
            .as_deref(),
        Some("on")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn omitted_models_keep_the_stored_inventory_and_empty_clears_it() {
    let dir = temp_dir("models-retention");
    let mut seeded = entry("p", Some("sk-1234567890abcdef"));
    seeded.models = Some(vec![
        ProviderModelInventoryItem {
            id: "glm-4".into(),
            context_window_tokens: None,
            supports_vision: None,
        },
        ProviderModelInventoryItem {
            id: "glm-5".into(),
            context_window_tokens: Some(128_000),
            supports_vision: Some(true),
        },
    ]);
    {
        let store = store(&dir);
        store.upsert(seeded, true).unwrap();
        // A partial edit (MCP tool / script) resends without `models`.
        let mut partial = entry("p", None);
        partial.model = "glm-4".into();
        store.upsert(partial, false).unwrap();
        assert_eq!(
            store.list().providers[0].models,
            vec![
                ProviderModelInventoryItem {
                    id: "glm-4".into(),
                    context_window_tokens: None,
                    supports_vision: None,
                },
                ProviderModelInventoryItem {
                    id: "glm-5".into(),
                    context_window_tokens: Some(128_000),
                    supports_vision: Some(true),
                },
            ]
        );
        // The settings form sends an explicit empty list: clear.
        let mut cleared = entry("p", None);
        cleared.model = "glm-4".into();
        cleared.models = Some(Vec::new());
        store.upsert(cleared, false).unwrap();
        assert_eq!(
            store.list().providers[0].models,
            Vec::<ProviderModelInventoryItem>::new()
        );
    }
}

#[test]
fn upsert_is_idempotent_and_the_document_round_trips() {
    let dir = temp_dir("roundtrip");
    {
        let store = store(&dir);
        store
            .upsert(entry("p", Some("sk-1234567890abcdef")), true)
            .unwrap();
        store
            .upsert(entry("p", Some("sk-1234567890abcdef")), true)
            .unwrap();
        assert_eq!(store.list().providers.len(), 1);
    }
    // A fresh store reads the same table back: memory and disk agree.
    let reopened = store(&dir);
    let list = reopened.list();
    assert_eq!(list.providers.len(), 1);
    assert_eq!(list.default_provider_id.as_deref(), Some("p"));
    assert_eq!(list.active_provider_id.as_deref(), Some("p"));
    let bytes = std::fs::read(dir.join("providers.json")).unwrap();
    assert!(
        !bytes.starts_with(&ark_common::UTF8_BOM_BYTES),
        "file must not carry a BOM"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn resolve_follows_the_default_and_the_explicit_id() {
    let dir = temp_dir("resolve");
    let store = store(&dir);
    store
        .upsert(entry("a", Some("sk-1234567890a")), true)
        .unwrap();
    store
        .upsert(entry("b", Some("sk-1234567890b")), false)
        .unwrap();
    assert_eq!(store.resolve(None).unwrap().provider_id, "a");
    assert_eq!(store.resolve(Some("b")).unwrap().provider_id, "b");
    assert!(store.resolve(Some("ghost")).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn resolve_vision_follows_the_same_selection_rules() {
    // Image parts are only sent on an explicit catalog-reported positive;
    // unknown (`None`) must read as "not declared", never as a guess.
    let dir = temp_dir("resolve-vision");
    let store = store(&dir);
    let mut vision = entry("v", Some("sk-1234567890v"));
    vision.supports_vision = Some(true);
    store.upsert(vision, true).unwrap();
    let mut blind = entry("b", Some("sk-1234567890b"));
    blind.supports_vision = Some(false);
    store.upsert(blind, true).unwrap();
    let mut unknown = entry("u", Some("sk-1234567890u"));
    unknown.supports_vision = None;
    store.upsert(unknown, true).unwrap();

    assert_eq!(store.resolve_vision(Some("v"), None), Some(true));
    assert_eq!(store.resolve_vision(Some("b"), None), Some(false));
    assert_eq!(store.resolve_vision(Some("u"), None), None);
    assert_eq!(store.resolve_vision(Some("ghost"), None), None);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Per-model metadata wins over the entry-level flag: metadata belongs to
/// the model id, so a thread override must read the override's flag.
#[test]
fn resolve_vision_prefers_the_inventory_entry_for_the_override_model() {
    let dir = temp_dir("resolve-vision-model");
    let store = store(&dir);
    let mut entry = entry("p", Some("sk-1234567890abcdef"));
    entry.supports_vision = Some(false);
    entry.models = Some(vec![ProviderModelInventoryItem {
        id: "vision-model".into(),
        context_window_tokens: Some(200_000),
        supports_vision: Some(true),
    }]);
    store.upsert(entry, true).unwrap();

    assert_eq!(store.resolve_vision(Some("p"), None), Some(false));
    assert_eq!(
        store.resolve_vision(Some("p"), Some("vision-model")),
        Some(true)
    );
    // An explicit override equal to the stored default model still reads
    // the entry-level flag (the inventory strips that id).
    assert_eq!(
        store.resolve_vision(Some("p"), Some("deepseek-chat")),
        Some(false)
    );
    // Unknown override model: no inventory row, so vision is unknown —
    // the default model's flag must not leak onto a different model id.
    assert_eq!(store.resolve_vision(Some("p"), Some("other")), None);
    let _ = std::fs::remove_dir_all(&dir);
}

/// An override model with an inventory entry compacts against the recorded
/// window; one without clears it, because the stored limit belongs to the
/// stored model id.
#[test]
fn resolve_for_model_applies_the_inventory_window() {
    let dir = temp_dir("resolve-for-model");
    let store = store(&dir);
    let mut entry = entry("p", Some("sk-1234567890abcdef"));
    entry.context_window_tokens = Some(128_000);
    entry.models = Some(vec![
        ProviderModelInventoryItem {
            id: "big".into(),
            context_window_tokens: Some(1_000_000),
            supports_vision: None,
        },
        ProviderModelInventoryItem {
            id: "small".into(),
            context_window_tokens: None,
            supports_vision: None,
        },
    ]);
    store.upsert(entry, true).unwrap();

    // No override: the stored model keeps its stored window.
    let base = store.resolve_for_model(Some("p"), None).unwrap();
    assert_eq!(base.context_window_tokens, Some(128_000));

    // Override with metadata: that model's window wins.
    let big = store
        .resolve_for_model(Some("p"), Some("big".to_string()))
        .unwrap();
    assert_eq!(big.model, "big");
    assert_eq!(big.context_window_tokens, Some(1_000_000));

    // Override without metadata: unknown, never another model's limit.
    let small = store
        .resolve_for_model(Some("p"), Some("small".to_string()))
        .unwrap();
    assert_eq!(small.context_window_tokens, None);

    // Unrelated override: also unknown.
    let other = store
        .resolve_for_model(Some("p"), Some("other".to_string()))
        .unwrap();
    assert_eq!(other.context_window_tokens, None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn resolve_is_none_without_a_key() {
    let dir = temp_dir("no-key");
    let store = store(&dir);
    store.upsert(entry("p", None), true).unwrap();
    assert!(store.resolve(None).is_none());
    let list = store.list();
    assert!(!list.providers[0].has_api_key);
    assert_eq!(list.providers[0].api_key_hint, None);
    // The default is still recorded, but nothing is bound to it.
    assert_eq!(list.default_provider_id.as_deref(), Some("p"));
    assert_eq!(list.active_provider_id, None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn testing_an_unknown_provider_does_not_hit_the_network() {
    let dir = temp_dir("probe-missing");
    let store = store(&dir);
    let response = store
        .test(ProviderTestRequest {
            provider_id: Some("ghost".into()),
            entry: None,
        })
        .await;
    assert!(!response.ok);
    assert_eq!(response.status, None);
    assert!(
        response.message.contains("does not exist"),
        "{}",
        response.message
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn testing_a_keyless_draft_is_rejected_before_any_request() {
    let dir = temp_dir("probe-no-key");
    let store = store(&dir);
    let response = store
        .test(ProviderTestRequest {
            provider_id: None,
            entry: Some(entry("draft", None)),
        })
        .await;
    assert!(!response.ok);
    assert!(
        response.message.contains("no api key"),
        "{}",
        response.message
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_draft_probe_reuses_the_stored_key() {
    let dir = temp_dir("probe-draft");
    let store = store(&dir);
    store
        .upsert(entry("p", Some("sk-1234567890")), false)
        .unwrap();
    // No key in the draft: the entry passed validation for the probe instead
    // of failing on a missing credential.
    let mut draft = entry("p", None);
    draft.base_url = "http://127.0.0.1:1/v1".into();
    let response = store
        .test(ProviderTestRequest {
            provider_id: None,
            entry: Some(draft),
        })
        .await;
    assert!(!response.ok);
    assert!(
        !response.message.contains("no api key"),
        "{}",
        response.message
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_corrupt_file_does_not_panic_on_open() {
    let dir = temp_dir("corrupt");
    std::fs::write(dir.join("providers.json"), b"{ not json").unwrap();
    let store = store(&dir);
    assert!(store.list().providers.is_empty());
    // A write repairs the file.
    store
        .upsert(entry("p", Some("sk-1234567890")), true)
        .unwrap();
    assert_eq!(store.list().providers.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_store_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ProviderStore>();
}
