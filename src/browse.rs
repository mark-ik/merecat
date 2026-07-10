//! The browse lane: address opening, fetch, metadata enrichment, favicon
//! discovery — and the fetch actor's adapter, converting the service's
//! concrete types into the app-owned [`Update`] messages so the vocabulary
//! stays port-agnostic. The folding itself is pure (testable; the app update
//! never blocks). Live content (the engine registry, document lifecycle,
//! verso-tile, content frames) is the separate `content` module born at
//! obviation rung 4: notes, gemini documents, local media, and HTML are all
//! content, while only some of it arrives by web fetching.

use fetch::{FetchCommand, FetchUpdate};
use mere::canvas::Canvas;

use crate::action::{Effect, FetchedPage, Update};

/// Convert one fetch-actor answer into the app vocabulary. `None` for
/// updates the app has no lane for yet (subresources arrive with `content`).
pub fn update_from_fetch(update: FetchUpdate) -> Option<Update> {
    match update {
        FetchUpdate::Page(outcome) => Some(Update::PageFetched {
            url: outcome.url,
            result: outcome.result.map(|fetched| FetchedPage {
                content_type: fetched.content_type,
                body: fetched.body,
            }),
        }),
        FetchUpdate::Favicon { owner_url, bytes } => {
            Some(Update::FaviconFetched { owner_url, bytes })
        }
        FetchUpdate::Subresource(_) => None,
    }
}

/// Translate an effect into the fetch actor's command, if it is fetch-shaped.
/// (The shell's effect runner calls this so the mapping stays beside the
/// enrichment it feeds.)
pub fn fetch_command_for(effect: &Effect) -> Option<FetchCommand> {
    match effect {
        Effect::FetchPage(url) => Some(FetchCommand::Page(url.clone())),
        Effect::FetchFavicon { owner_url, url } => Some(FetchCommand::Favicon {
            owner_url: owner_url.clone(),
            url: url.clone(),
        }),
        _ => None,
    }
}

/// Fold one completed page fetch into the graph: stamp the response's
/// Content-Type as the node's MIME hint, and for HTML extract the page
/// `<title>` (render-free static parse) so the canvas caption flips from the
/// host fallback to the real title, then chase the page's favicon so the node
/// face wears a real icon.
pub fn apply_page(
    canvas: &mut Canvas,
    url: String,
    result: Result<FetchedPage, String>,
) -> Vec<Effect> {
    let mut effects = Vec::new();
    match result {
        Ok(fetched) => {
            let media = fetched
                .content_type
                .as_deref()
                .and_then(|ct| ct.split(';').next())
                .map(|m| m.trim().to_ascii_lowercase());
            tracing::info!(%url, content_type = ?media, bytes = fetched.body.len(), "page fetched");
            canvas.set_node_mime_hint(&url, media.clone());
            if media.as_deref() == Some("text/html") {
                let doc = serval_static_dom::StaticDocument::parse(&fetched.body);
                if let Some(title) = serval_extract::extract(&doc).title {
                    if canvas.set_node_title(&url, title.clone()) {
                        tracing::info!(%url, %title, "node title enriched from the page");
                    }
                }
                // Best-effort: chase the page's favicon; the bytes route back
                // as a FaviconFetched update keyed to this page url.
                if let Some(icon_url) = favicon_url_for(&url, &doc) {
                    effects.push(Effect::FetchFavicon {
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

/// A page's favicon arrived: decode it to RGBA and stamp it on the node
/// currently at the owner url; the canvas paints it (inset, accent-framed) on
/// the next frame.
pub fn apply_favicon(canvas: &mut Canvas, owner_url: &str, bytes: &[u8]) -> Vec<Effect> {
    let Some(decoded) = serval_layout::decode_image_bytes(bytes) else {
        return Vec::new();
    };
    if canvas.set_node_favicon(owner_url, decoded.rgba, decoded.width, decoded.height) {
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
