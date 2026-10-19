//! Keep lightweight, immutable image references in history/checkpoints. Resolve
//! pixels only for an outgoing request, never into saved conversations.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;

const PREFIX: &str = "argus-chat-image:";
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Reference {
    conversation_id: String,
    file: String,
}

pub fn reference(conversation_id: &str, file: &str) -> String {
    let data = Reference {
        conversation_id: conversation_id.into(),
        file: file.into(),
    };
    format!(
        "{PREFIX}{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&data).expect("image reference"))
    )
}

pub async fn for_request(
    root: &str,
    messages: &[Value],
    vision: bool,
) -> Result<Vec<Value>, String> {
    let root = root.to_string();
    let messages = messages.to_vec();
    tokio::task::spawn_blocking(move || resolve(&root, messages, vision))
        .await
        .map_err(|e| format!("Restore conversation images: {e}"))?
}

fn resolve(root: &str, mut messages: Vec<Value>, vision: bool) -> Result<Vec<Value>, String> {
    let mut loaded = HashMap::new();
    for message in &mut messages {
        let Some(parts) = message["content"].as_array_mut() else {
            continue;
        };
        for part in parts {
            let Some(url) = part["image_url"]["url"].as_str() else {
                continue;
            };
            let Some(encoded) = url.strip_prefix(PREFIX) else {
                continue;
            };
            if encoded.len() > 8192 {
                return Err("Invalid saved conversation image reference.".into());
            }
            let data = URL_SAFE_NO_PAD
                .decode(encoded)
                .map_err(|_| "Invalid saved conversation image reference.")?;
            let reference: Reference = serde_json::from_slice(&data)
                .map_err(|_| "Invalid saved conversation image reference.")?;
            if !vision {
                *part = json!({"type":"text","text":"[A previously inspected paper image is saved with this conversation, but the selected model cannot see images. Use its accompanying text; choose a vision-capable model to inspect the figure. Do not claim to see it.]"});
                continue;
            }
            let image = loaded.entry(url.to_string()).or_insert_with(|| {
                crate::copilot::read_conversation_image(
                    root,
                    &reference.conversation_id,
                    &reference.file,
                )
                .ok()
            });
            if let Some(image) = image {
                part["image_url"]["url"] = json!(image);
            } else {
                // A deleted/missing file must not permanently block chat or
                // masquerade as pixels. The adjacent source/page label lets
                // the agent read it again with its ordinary page-view tool.
                *part = json!({"type":"text","text":"[The previously inspected image labelled immediately above could not be restored from this library. Its pixels are unavailable in this request. Reopen that paper/page with view_paper_page if needed; do not claim you can currently see the image.]"});
            }
        }
    }
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn immutable_pixels_return_on_every_followup_but_stay_out_of_checkpoints() {
        let root =
            std::env::temp_dir().join(format!("argus-image-replay-{}", uuid::Uuid::new_v4()));
        let file = "page.png";
        // Safe ASCII conversation ids map to the same on-disk stem.
        std::fs::create_dir_all(root.join("chats/conversation")).unwrap();
        let png = b"\x89PNG\r\n\x1a\nimmutable saved pixels";
        std::fs::write(root.join("chats/conversation").join(file), png).unwrap();
        let saved = vec![json!({"role":"user","content":[
            {"type":"text","text":"Paper p, PDF page 15, crop of figure 4"},
            {"type":"image_url","image_url":{"url":reference("conversation",file)}}]})];
        let disk = serde_json::to_string(&saved).unwrap();
        for _ in 0..2 {
            let replay = for_request(root.to_str().unwrap(), &saved, true)
                .await
                .unwrap();
            let expected = format!(
                "data:image/png;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(png)
            );
            assert_eq!(replay[0]["content"][1]["image_url"]["url"], expected);
            assert_eq!(serde_json::to_string(&saved).unwrap(), disk);
            assert!(!disk.contains("data:image/png"));
        }
        let no_vision = for_request(root.to_str().unwrap(), &saved, false)
            .await
            .unwrap();
        assert!(no_vision[0]["content"][1]["text"]
            .as_str()
            .unwrap()
            .contains("cannot see images"));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn missing_files_and_traversal_never_become_outbound_image_urls() {
        let saved = vec![json!({"role":"user","content":[
            {"type":"image_url","image_url":{"url":reference("chat","../../secret.png")}},
            {"type":"image_url","image_url":{"url":reference("chat","missing.png")}}]})];
        let replay = for_request("/nonexistent-argus-library", &saved, true)
            .await
            .unwrap();
        assert!(replay[0]["content"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["type"] == "text"));
        assert!(serde_json::to_string(&replay)
            .unwrap()
            .contains("could not be restored"));
        let malformed = vec![
            json!({"role":"user","content":[{"type":"image_url","image_url":{"url":"argus-chat-image:invalid"}}]}),
        ];
        assert!(for_request("/unused", &malformed, true).await.is_err());
    }
}
