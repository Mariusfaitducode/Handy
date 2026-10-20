//! Notion destination: each note becomes a child page of a page the user
//! shared with their internal integration.

use super::text::{body_blocks, BodyBlock, NoteDraft};
use serde_json::{json, Value};
use std::time::Duration;

const API: &str = "https://api.notion.com/v1";
const NOTION_VERSION: &str = "2022-06-28";
/// Notion accepts at most 100 children per request.
const MAX_CHILDREN: usize = 100;

pub const ERR_INVALID_TOKEN: &str = "notion_invalid_token";
pub const ERR_PAGE_NOT_FOUND: &str = "notion_page_not_found";
pub const ERR_NETWORK: &str = "notion_network";
pub const ERR_API: &str = "notion_api";
pub const ERR_TOKEN_MISSING: &str = "notion_token_missing";

pub struct CreatedPage {
    pub url: String,
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| format!("{ERR_NETWORK}: {e}"))
}

fn request(builder: reqwest::RequestBuilder, token: &str) -> reqwest::RequestBuilder {
    builder
        .bearer_auth(token.trim())
        .header("Notion-Version", NOTION_VERSION)
}

/// Map a non-success Notion response to one of our error codes.
async fn error_for(response: reqwest::Response) -> String {
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    let code = body.get("code").and_then(Value::as_str).unwrap_or("");
    let message = body.get("message").and_then(Value::as_str).unwrap_or("");
    match (status.as_u16(), code) {
        (401, _) | (_, "unauthorized") => ERR_INVALID_TOKEN.to_string(),
        (404, _) | (_, "object_not_found") | (403, _) | (_, "restricted_resource") => {
            ERR_PAGE_NOT_FOUND.to_string()
        }
        _ => format!("{ERR_API}: {} {message}", status.as_u16()),
    }
}

fn network_error(e: reqwest::Error) -> String {
    format!("{ERR_NETWORK}: {e}")
}

fn plain_title(page: &Value) -> Option<String> {
    // The title property's name varies (`title`, `Name`…); find it by type.
    let properties = page.get("properties")?.as_object()?;
    let title = properties
        .values()
        .find(|p| p.get("type").and_then(Value::as_str) == Some("title"))?;
    let text: String = title
        .get("title")?
        .as_array()?
        .iter()
        .filter_map(|t| t.get("plain_text").and_then(Value::as_str))
        .collect();
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Check the token can read the parent page, and return its title.
pub async fn verify_parent(token: &str, page_id: &str) -> Result<Option<String>, String> {
    let response = request(client()?.get(format!("{API}/pages/{page_id}")), token)
        .send()
        .await
        .map_err(network_error)?;
    if !response.status().is_success() {
        return Err(error_for(response).await);
    }
    let page: Value = response.json().await.map_err(network_error)?;
    Ok(plain_title(&page))
}

fn rich_text(text: &str) -> Value {
    json!([{ "type": "text", "text": { "content": text } }])
}

fn children(draft: &NoteDraft) -> Vec<Value> {
    body_blocks(&draft.body)
        .into_iter()
        .map(|block| match block {
            BodyBlock::Paragraph(text) => json!({
                "object": "block",
                "type": "paragraph",
                "paragraph": { "rich_text": rich_text(&text) }
            }),
            BodyBlock::Bullet(text) => json!({
                "object": "block",
                "type": "bulleted_list_item",
                "bulleted_list_item": { "rich_text": rich_text(&text) }
            }),
        })
        .collect()
}

pub async fn create_page(
    token: &str,
    parent_page_id: &str,
    draft: &NoteDraft,
) -> Result<CreatedPage, String> {
    let blocks = children(draft);
    let mut batches = blocks.chunks(MAX_CHILDREN);
    let first: Vec<Value> = batches.next().map(<[Value]>::to_vec).unwrap_or_default();
    let body = json!({
        "parent": { "page_id": parent_page_id },
        "properties": {
            "title": { "title": rich_text(&draft.title) }
        },
        "children": first,
    });
    let response = request(client()?.post(format!("{API}/pages")), token)
        .json(&body)
        .send()
        .await
        .map_err(network_error)?;
    if !response.status().is_success() {
        return Err(error_for(response).await);
    }
    let page: Value = response.json().await.map_err(network_error)?;
    let url = page
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    // Long notes: append the remaining blocks in batches of 100.
    if let Some(page_id) = page.get("id").and_then(Value::as_str) {
        for batch in batches {
            let response = request(
                client()?.patch(format!("{API}/blocks/{page_id}/children")),
                token,
            )
            .json(&json!({ "children": batch }))
            .send()
            .await
            .map_err(network_error)?;
            if !response.status().is_success() {
                return Err(error_for(response).await);
            }
        }
    }
    Ok(CreatedPage { url })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_is_found_whatever_the_property_name() {
        let page = json!({
            "properties": {
                "Tags": { "type": "multi_select" },
                "Name": { "type": "title", "title": [
                    { "plain_text": "Jour" }, { "plain_text": "nal" }
                ]}
            }
        });
        assert_eq!(plain_title(&page).as_deref(), Some("Journal"));
        assert_eq!(plain_title(&json!({})), None);
    }

    #[test]
    fn children_keep_every_line_and_are_typed() {
        let draft = NoteDraft {
            title: "t".into(),
            body: (0..150)
                .map(|i| format!("- item {i}"))
                .collect::<Vec<_>>()
                .join("\n"),
        };
        let blocks = children(&draft);
        assert_eq!(blocks.len(), 150);
        assert_eq!(blocks.chunks(MAX_CHILDREN).count(), 2);
        assert_eq!(blocks[0]["type"], "bulleted_list_item");
        assert_eq!(
            blocks[0]["bulleted_list_item"]["rich_text"][0]["text"]["content"],
            "item 0"
        );
    }
}
