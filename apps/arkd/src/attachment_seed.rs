//! Turn-seed attachment resolution (attachment lifecycle v2, phase 2).
//!
//! Pure decision logic, deliberately free of I/O beyond the `AttachmentStore`
//! port: the composition root calls it once per turn with the ids the route
//! already authorized against the thread's association events. The agent never
//! reads the filesystem — it only receives message parts (design
//! requirements/advanced-local-capabilities.md, 附件生命周期 v2 第二期).

use std::sync::{Arc, Mutex};

use ark_common::AttachmentId;
use ark_contract::{AttachmentEntry, TurnAttachmentDelivery, TurnAttachmentOutcome};
use ark_ports::{AttachmentStore, ModelImage};

use base64::Engine as _;

/// Per-turn total image budget in bytes. Individual attachments are already
/// capped at the 25 MiB upload limit; this bounds how much of the model
/// context one turn's images can consume.
pub const IMAGE_BUDGET_BYTES: u64 = 20 * 1024 * 1024;

/// Everything a turn's attachments contribute to the model seed.
#[derive(Debug, Default, PartialEq)]
pub struct AttachmentSeed {
    /// Images that ride the seed as multimodal parts, in attachment order.
    pub images: Vec<ModelImage>,
    /// Composed preview text for non-image, degraded, skipped and unavailable
    /// attachments; empty when there is nothing to say. Rides the user message
    /// content so it also survives history replay.
    pub preview: String,
    /// Per-attachment delivery record for the durable `turn_started` event.
    pub outcomes: Vec<TurnAttachmentOutcome>,
}

/// Mutable image budget shared by the initial seed and every steer in one turn.
/// A fresh instance is created at turn assembly; cloning it shares the counter.
#[derive(Debug, Clone)]
pub struct AttachmentBudget {
    remaining: Arc<Mutex<u64>>,
}

impl AttachmentBudget {
    pub fn new(limit: u64) -> Self {
        Self {
            remaining: Arc::new(Mutex::new(limit)),
        }
    }

    pub fn resolve(
        &self,
        store: &dyn AttachmentStore,
        ids: &[AttachmentId],
        vision: bool,
    ) -> AttachmentSeed {
        let mut remaining = self
            .remaining
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        resolve_attachments_with_remaining(store, ids, vision, &mut remaining)
    }
}

fn resolve_attachments_with_remaining(
    store: &dyn AttachmentStore,
    ids: &[AttachmentId],
    vision: bool,
    remaining: &mut u64,
) -> AttachmentSeed {
    let mut images = Vec::new();
    let mut previews: Vec<String> = Vec::new();
    let mut outcomes = Vec::new();
    for id in ids {
        match store.get(*id) {
            Err(error) => {
                previews.push(format!("[attachment {id}: unavailable]"));
                outcomes.push(TurnAttachmentOutcome {
                    attachment_id: *id,
                    original_filename: None,
                    delivered: TurnAttachmentDelivery::Unavailable,
                    detail: Some(error.message),
                });
            }
            Ok(entry) => {
                let mime = entry.mime_type.clone().unwrap_or_default();
                if !mime.starts_with("image/") {
                    previews.push(preview_line(&entry));
                    outcomes.push(TurnAttachmentOutcome {
                        attachment_id: *id,
                        original_filename: Some(entry.original_filename.clone()),
                        delivered: TurnAttachmentDelivery::PreviewText,
                        detail: None,
                    });
                    continue;
                }
                if !vision {
                    previews.push(preview_line(&entry));
                    outcomes.push(TurnAttachmentOutcome {
                        attachment_id: *id,
                        original_filename: Some(entry.original_filename.clone()),
                        delivered: TurnAttachmentDelivery::Degraded,
                        detail: Some("model did not declare vision support".into()),
                    });
                    continue;
                }
                if entry.byte_size > *remaining {
                    previews.push(format!(
                        "[attachment {}: skipped, image exceeds the remaining per-turn image budget]",
                        entry.original_filename
                    ));
                    outcomes.push(TurnAttachmentOutcome {
                        attachment_id: *id,
                        original_filename: Some(entry.original_filename.clone()),
                        delivered: TurnAttachmentDelivery::Skipped,
                        detail: Some(format!(
                            "image of {} bytes exceeds the remaining per-turn budget",
                            entry.byte_size
                        )),
                    });
                    continue;
                }
                match store.read(*id) {
                    Ok(bytes) => {
                        *remaining -= bytes.len() as u64;
                        images.push(ModelImage {
                            mime_type: mime,
                            data: base64::engine::general_purpose::STANDARD.encode(bytes),
                        });
                        outcomes.push(TurnAttachmentOutcome {
                            attachment_id: *id,
                            original_filename: Some(entry.original_filename.clone()),
                            delivered: TurnAttachmentDelivery::Image,
                            detail: None,
                        });
                    }
                    Err(error) => {
                        previews.push(format!("[attachment {id}: unavailable]"));
                        outcomes.push(TurnAttachmentOutcome {
                            attachment_id: *id,
                            original_filename: Some(entry.original_filename.clone()),
                            delivered: TurnAttachmentDelivery::Unavailable,
                            detail: Some(error.message),
                        });
                    }
                }
            }
        }
    }
    AttachmentSeed {
        images,
        preview: previews.join("\n"),
        outcomes,
    }
}

/// One preview line for an attachment that cannot ride the seed as an image.
fn preview_line(entry: &AttachmentEntry) -> String {
    match &entry.preview_text {
        Some(text) if !text.trim().is_empty() => {
            format!("[attachment: {}]\n{}", entry.original_filename, text)
        }
        _ => format!(
            "[attachment: {} (no text preview)]",
            entry.original_filename
        ),
    }
}

/// Build the reactor's per-entry steer resolver on the turn's shared budget.
/// Images and outcomes come from the same resolution, so the durable item
/// records exactly what the model request received.
pub fn steer_attachment_resolver(
    store: Arc<dyn AttachmentStore>,
    vision: bool,
    budget: AttachmentBudget,
) -> ark_agent::SteerAttachmentResolver {
    Arc::new(move |ids: &[ark_common::AttachmentId]| {
        let seed = budget.resolve(store.as_ref(), ids, vision);
        ark_agent::SteerAttachments {
            images: seed.images,
            outcomes: seed.outcomes,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_common::{ArkError, AttachmentId as Id};
    use ark_contract::AttachmentDiagnostics;
    use std::collections::HashSet;

    /// In-memory store with one missing id to exercise the failure path.
    struct FakeStore {
        entries: Vec<AttachmentEntry>,
        bytes: Vec<(Id, Vec<u8>)>,
    }

    impl FakeStore {
        fn with_images(images: &[(&str, usize)]) -> Self {
            let entries = images
                .iter()
                .map(|(id, size)| AttachmentEntry {
                    id: id.parse().unwrap(),
                    original_filename: format!("{id}.png"),
                    byte_size: *size as u64,
                    mime_type: Some("image/png".into()),
                    created_at: chrono::Utc::now(),
                    preview_text: None,
                    sha256: String::new(),
                })
                .collect();
            let bytes = images
                .iter()
                .map(|(id, size)| ((*id).parse().unwrap(), vec![0u8; *size]))
                .collect();
            Self { entries, bytes }
        }

        fn with_image(id: &str, mime: &str, size: usize, preview: Option<&str>) -> Self {
            let id: Id = id.parse().unwrap();
            Self {
                entries: vec![AttachmentEntry {
                    id,
                    original_filename: "pic.png".into(),
                    byte_size: size as u64,
                    mime_type: Some(mime.into()),
                    created_at: chrono::Utc::now(),
                    preview_text: preview.map(str::to_string),
                    sha256: String::new(),
                }],
                bytes: vec![(id, vec![0u8; size])],
            }
        }
    }

    impl AttachmentStore for FakeStore {
        fn max_bytes(&self) -> u64 {
            25 * 1024 * 1024
        }
        fn upload(
            &self,
            _upload: ark_ports::AttachmentUpload,
        ) -> ark_common::Result<AttachmentEntry> {
            unimplemented!()
        }
        fn get(&self, id: Id) -> ark_common::Result<AttachmentEntry> {
            self.entries
                .iter()
                .find(|entry| entry.id == id)
                .cloned()
                .ok_or_else(|| ArkError::not_found("attachment not found"))
        }
        fn read(&self, id: Id) -> ark_common::Result<Vec<u8>> {
            self.bytes
                .iter()
                .find(|(key, _)| *key == id)
                .map(|(_, bytes)| bytes.clone())
                .ok_or_else(|| ArkError::not_found("attachment bytes missing"))
        }
        fn delete(&self, _id: Id) -> ark_common::Result<()> {
            unimplemented!()
        }
        fn diagnostics(
            &self,
            _associated: &HashSet<Id>,
        ) -> ark_common::Result<AttachmentDiagnostics> {
            unimplemented!()
        }
    }

    fn image_id() -> Id {
        "019689f4-8000-7000-8000-000000000001".parse().unwrap()
    }

    #[test]
    fn a_vision_capable_model_receives_the_image_within_the_budget() {
        let store =
            FakeStore::with_image("019689f4-8000-7000-8000-000000000001", "image/png", 4, None);
        let seed = AttachmentBudget::new(1_000).resolve(&store, &[image_id()], true);
        assert_eq!(seed.images.len(), 1);
        assert_eq!(seed.images[0].mime_type, "image/png");
        assert!(seed.preview.is_empty());
        assert_eq!(seed.outcomes[0].delivered, TurnAttachmentDelivery::Image);
    }

    #[test]
    fn an_undeclared_capability_degrades_the_image_to_its_preview() {
        let store = FakeStore::with_image(
            "019689f4-8000-7000-8000-000000000001",
            "image/png",
            4,
            Some("alt text"),
        );
        let seed = AttachmentBudget::new(1_000).resolve(&store, &[image_id()], false);
        assert!(seed.images.is_empty());
        assert!(seed.preview.contains("alt text"));
        assert_eq!(seed.outcomes[0].delivered, TurnAttachmentDelivery::Degraded);
    }

    #[test]
    fn an_image_over_the_remaining_budget_is_skipped_in_order() {
        let store =
            FakeStore::with_image("019689f4-8000-7000-8000-000000000001", "image/png", 8, None);
        let seed = AttachmentBudget::new(4).resolve(&store, &[image_id()], true);
        assert!(seed.images.is_empty());
        assert_eq!(seed.outcomes[0].delivered, TurnAttachmentDelivery::Skipped);
        assert!(seed.preview.contains("budget"));
    }

    #[test]
    fn a_non_image_attachment_rides_as_preview_text() {
        let store = FakeStore::with_image(
            "019689f4-8000-7000-8000-000000000001",
            "text/plain",
            4,
            Some("file body"),
        );
        let seed = AttachmentBudget::new(1_000).resolve(&store, &[image_id()], true);
        assert!(seed.images.is_empty());
        assert!(seed.preview.contains("file body"));
        assert_eq!(
            seed.outcomes[0].delivered,
            TurnAttachmentDelivery::PreviewText
        );
    }

    #[test]
    fn a_missing_attachment_is_unavailable_without_failing_the_turn() {
        let store =
            FakeStore::with_image("019689f4-8000-7000-8000-000000000001", "image/png", 4, None);
        let ghost: Id = "019689f4-8000-7000-8000-000000000002".parse().unwrap();
        let seed = AttachmentBudget::new(1_000).resolve(&store, &[ghost, image_id()], true);
        assert_eq!(
            seed.outcomes[0].delivered,
            TurnAttachmentDelivery::Unavailable
        );
        // The later healthy image is still delivered.
        assert_eq!(seed.images.len(), 1);
    }

    #[test]
    fn successive_steer_resolutions_share_one_per_turn_budget() {
        let first: Id = "019689f4-8000-7000-8000-000000000001".parse().unwrap();
        let second: Id = "019689f4-8000-7000-8000-000000000002".parse().unwrap();
        let store = Arc::new(FakeStore::with_images(&[
            ("019689f4-8000-7000-8000-000000000001", 6),
            ("019689f4-8000-7000-8000-000000000002", 6),
        ]));
        let resolve = steer_attachment_resolver(store, true, AttachmentBudget::new(10));

        assert_eq!(resolve(&[first]).images.len(), 1);
        assert!(
            resolve(&[second]).images.is_empty(),
            "the second correction must not reset the turn's image budget"
        );
    }
}
