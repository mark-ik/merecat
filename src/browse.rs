//! The browse lane: address opening, fetch, metadata enrichment, favicon
//! discovery — and the fetch actor's adapter, converting the service's
//! concrete types into the app-owned [`Update`] messages so the vocabulary
//! stays port-agnostic. The folding itself is pure (testable; the app update
//! never blocks). Live content (the engine registry, document lifecycle,
//! verso-tile, content frames) is the separate `content` module born at
//! obviation rung 4: notes, gemini documents, local media, and HTML are all
//! content, while only some of it arrives by web fetching.
//!
//! Correlation-over-URLs (the architecture plan's recorded decision, landed
//! at this enrichment touch): effects carry the REQUESTING node's id, the
//! shell notes it in [`PendingFetches`] before commanding the actor, and the
//! adapter reattaches it on the way back — the fetch crate's wire types stay
//! untouched (meerkat shares them). Two nodes fetching the same URL are
//! interchangeable attributions (same URL, same payload), so the pending
//! table keys by URL and pops one requester per completion. What the id
//! buys is the STAMP side: enrichment lands on the exact requester via the
//! canvas's member-keyed setters, and a late result against a node that
//! navigated away drops explicitly instead of repainting the wrong page's
//! face (`get_node_by_url` first-match was the old, wrong answer).

use std::collections::HashMap;

use fetch::{FetchCommand, FetchUpdate};
use mere::canvas::Canvas;
use uuid::Uuid;

use crate::action::{Effect, FetchedPage, Update};

/// The shell-owned correlation table: which node asked for each in-flight
/// fetch. Port bookkeeping, not app truth (the app's record is the effect it
/// emitted); keyed by URL because that is all the actor echoes back.
#[derive(Debug, Default)]
pub struct PendingFetches {
    pages: HashMap<String, Vec<Uuid>>,
    /// Keyed by owner page URL (the actor echoes `owner_url`, not the icon URL).
    favicons: HashMap<String, Vec<Uuid>>,
}

impl PendingFetches {
    pub fn note_page(&mut self, url: &str, node: Uuid) {
        self.pages.entry(url.to_string()).or_default().push(node);
    }

    pub fn note_favicon(&mut self, owner_url: &str, node: Uuid) {
        self.favicons
            .entry(owner_url.to_string())
            .or_default()
            .push(node);
    }

    fn take_page(&mut self, url: &str) -> Option<Uuid> {
        take_one(&mut self.pages, url)
    }

    fn take_favicon(&mut self, owner_url: &str) -> Option<Uuid> {
        take_one(&mut self.favicons, owner_url)
    }
}

fn take_one(map: &mut HashMap<String, Vec<Uuid>>, key: &str) -> Option<Uuid> {
    let list = map.get_mut(key)?;
    let node = list.pop();
    if list.is_empty() {
        map.remove(key);
    }
    node
}

/// Convert one fetch-actor answer into the app vocabulary, reattaching the
/// requesting node from the pending table. `None` for updates the app has no
/// lane for (subresources), or answers nothing asked for (an unmatched
/// completion is logged, not guessed at).
pub fn update_from_fetch(update: FetchUpdate, pending: &mut PendingFetches) -> Option<Update> {
    match update {
        FetchUpdate::Page(outcome) => {
            let Some(node) = pending.take_page(&outcome.url) else {
                tracing::warn!(url = %outcome.url, "page completion without a pending requester; dropped");
                return None;
            };
            Some(Update::PageFetched {
                node,
                url: outcome.url,
                result: outcome.result.map(|fetched| FetchedPage {
                    content_type: fetched.content_type,
                    body: fetched.body,
                }),
            })
        }
        FetchUpdate::Favicon { owner_url, bytes } => {
            let Some(node) = pending.take_favicon(&owner_url) else {
                tracing::warn!(url = %owner_url, "favicon completion without a pending requester; dropped");
                return None;
            };
            Some(Update::FaviconFetched {
                node,
                owner_url,
                bytes,
            })
        }
        FetchUpdate::Subresource(_) => None,
    }
}

/// Translate an effect into the fetch actor's command, if it is fetch-shaped.
/// The shell notes the requester in [`PendingFetches`] and commands the
/// actor; the mapping stays beside the enrichment it feeds.
pub fn fetch_command_for(effect: &Effect, pending: &mut PendingFetches) -> Option<FetchCommand> {
    match effect {
        Effect::FetchPage { node, url } => {
            pending.note_page(url, *node);
            Some(FetchCommand::Page(url.clone()))
        }
        Effect::FetchFavicon {
            node,
            owner_url,
            url,
        } => {
            pending.note_favicon(owner_url, *node);
            Some(FetchCommand::Favicon {
                owner_url: owner_url.clone(),
                url: url.clone(),
            })
        }
        _ => None,
    }
}

/// Whether `node` still lives at `url` — the staleness gate. Enrichment
/// belongs to the page that was fetched; a node that has navigated away (or
/// died) since the request drops the late result explicitly.
fn still_current(canvas: &Canvas, node: Uuid, url: &str) -> bool {
    canvas
        .graph()
        .get_node_by_id(node)
        .is_some_and(|(_, n)| n.url() == url)
}

/// Fold one completed page fetch into the graph: stamp the response's
/// Content-Type as the requesting node's MIME hint, and for HTML extract the
/// page `<title>` (render-free static parse) so the canvas caption flips from
/// the host fallback to the real title, then chase the page's favicon so the
/// node face wears a real icon. All stamps target the requester by member id.
pub fn apply_page(
    canvas: &mut Canvas,
    node: Uuid,
    url: String,
    result: Result<FetchedPage, String>,
) -> Vec<Effect> {
    if !still_current(canvas, node, &url) {
        tracing::info!(%node, %url, "page result for a superseded node; dropped");
        return Vec::new();
    }
    let mut effects = Vec::new();
    match result {
        Ok(fetched) => {
            let media = fetched
                .content_type
                .as_deref()
                .and_then(|ct| ct.split(';').next())
                .map(|m| m.trim().to_ascii_lowercase());
            tracing::info!(%url, content_type = ?media, bytes = fetched.body.len(), "page fetched");
            canvas.set_node_mime_hint_for(node, media.clone());
            if media.as_deref() == Some("text/html") {
                let doc = serval_static_dom::StaticDocument::parse(&fetched.body);
                if let Some(title) = serval_extract::extract(&doc).title {
                    if canvas.set_node_title_for(node, title.clone()) {
                        tracing::info!(%url, %title, "node title enriched from the page");
                    }
                }
                // Best-effort: chase the page's favicon; the bytes route back
                // as a FaviconFetched update correlated to this node.
                if let Some(icon_url) = favicon_url_for(&url, &doc) {
                    effects.push(Effect::FetchFavicon {
                        node,
                        owner_url: url.clone(),
                        url: icon_url,
                    });
                }
            }
        }
        Err(err) => {
            tracing::warn!(%url, %err, "page fetch failed");
        }
    }
    effects.push(Effect::SaveSession);
    effects.push(Effect::Redraw);
    effects
}

/// A page's favicon arrived: decode it to RGBA and stamp it on the
/// requesting node, if it still lives at the page the icon belongs to.
pub fn apply_favicon(
    canvas: &mut Canvas,
    node: Uuid,
    owner_url: &str,
    bytes: &[u8],
) -> Vec<Effect> {
    if !still_current(canvas, node, owner_url) {
        tracing::info!(%node, url = %owner_url, "favicon for a superseded node; dropped");
        return Vec::new();
    }
    let Some(decoded) = serval_layout::decode_image_bytes(bytes) else {
        return Vec::new();
    };
    if canvas.set_node_favicon_for(node, decoded.rgba, decoded.width, decoded.height) {
        tracing::info!(url = %owner_url, "node favicon enriched from the page");
        vec![Effect::SaveSession, Effect::Redraw]
    } else {
        Vec::new()
    }
}

/// The favicon URL for a fetched page: the document's declared
/// `<link rel=icon>` href resolved against the page URL, else the well-known
/// `/favicon.ico` for web pages. `None` when neither applies.
fn favicon_url_for(page_url: &str, doc: &serval_static_dom::StaticDocument) -> Option<String> {
    let base = url::Url::parse(page_url).ok()?;
    if let Some(href) = serval_layout::linked_icon_href(doc) {
        if let Ok(resolved) = base.join(&href) {
            return Some(resolved.to_string());
        }
    }
    if matches!(base.scheme(), "http" | "https") {
        if let Ok(fallback) = base.join("/favicon.ico") {
            return Some(fallback.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two nodes share one URL; the stamp lands on the CORRELATED node, not
    /// whichever `get_node_by_url` answers first (the bug this lane fixes).
    #[test]
    fn enrichment_stamps_the_requesting_node_among_url_twins() {
        let mut canvas = Canvas::new();
        let first_key = canvas.visit("https://elsewhere.example");
        let first = canvas.graph().get_node(first_key).unwrap().id;
        let target_key = canvas.visit("https://twin.example");
        let target = canvas.graph().get_node(target_key).unwrap().id;
        // Navigate the first node onto the same URL: two nodes, one address.
        assert!(canvas.navigate_member(first, "https://twin.example"));

        let effects = apply_page(
            &mut canvas,
            target,
            "https://twin.example".to_string(),
            Ok(FetchedPage {
                content_type: Some("text/html".to_string()),
                body: "<html><head><title>Twin B</title></head></html>".to_string(),
            }),
        );
        assert!(!effects.is_empty());
        let titles: Vec<_> = canvas
            .graph()
            .nodes()
            .map(|(_, n)| (n.id, n.title.clone()))
            .collect();
        for (id, title) in titles {
            if id == target {
                assert_eq!(title, "Twin B", "the requester is enriched");
            } else {
                assert_ne!(title, "Twin B", "its URL twin is untouched");
            }
        }
    }

    /// A late result against a node that navigated away drops explicitly.
    #[test]
    fn superseded_node_drops_the_late_result() {
        let mut canvas = Canvas::new();
        let key = canvas.visit("https://before.example");
        let node = canvas.graph().get_node(key).unwrap().id;
        canvas.navigate_member(node, "https://after.example");

        let effects = apply_page(
            &mut canvas,
            node,
            "https://before.example".to_string(),
            Ok(FetchedPage {
                content_type: Some("text/html".to_string()),
                body: "<html><head><title>Stale</title></head></html>".to_string(),
            }),
        );
        assert!(effects.is_empty(), "no stamps, no save, no redraw");
        let (_, n) = canvas.graph().get_node_by_id(node).unwrap();
        assert_ne!(n.title, "Stale", "the stale title never landed");
    }

    /// The pending table pops one requester per completion and never guesses
    /// at an unmatched answer.
    #[test]
    fn pending_table_correlates_and_refuses_to_guess() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let mut pending = PendingFetches::default();
        pending.note_page("https://x.example", a);
        pending.note_page("https://x.example", b);
        assert!(pending.take_page("https://x.example").is_some());
        assert!(pending.take_page("https://x.example").is_some());
        assert!(pending.take_page("https://x.example").is_none());

        let unmatched = update_from_fetch(
            FetchUpdate::Favicon {
                owner_url: "https://nobody.example".to_string(),
                bytes: vec![1, 2, 3],
            },
            &mut pending,
        );
        assert!(unmatched.is_none(), "an unmatched completion is dropped, not guessed");
    }
}
