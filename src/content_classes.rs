//! Turnstone's built-in content classes, defined through the same data and
//! schema seams a pack uses.

use chartulary::{
    CLASS_FACET, ClassError, ClassMembership, ClassRegistry, ContentClass, FacetError, FacetId,
};
use eidetic::{MereNativeFieldSpec, MereNativeSchemaBuilder, SchemaDefinition, SchemaFormat};
use serde_json::json;
use session_runtime::SchemaFacetValidator;

pub(crate) const WEB_PAGE_CLASS: &str = "turnstone.web-page";
pub(crate) const NOTE_CLASS: &str = "turnstone.note";
pub(crate) const WEB_PAGE_FACET: &str = "web.page";
pub(crate) const NOTE_DOCUMENT_FACET: &str = "note.document";

/// The class/schema set Turnstone ships. Nothing here is privileged in
/// chartulary: a pack can construct and register the same data types.
pub(crate) struct BuiltinContentClasses {
    pub registry: ClassRegistry,
    pub validator: SchemaFacetValidator,
}

impl BuiltinContentClasses {
    pub fn new() -> Self {
        let mut validator = SchemaFacetValidator::new();
        validator.register(
            FacetId::new(WEB_PAGE_FACET),
            MereNativeSchemaBuilder::new("turnstone.web-page/v1")
                .description("Derived web-page profile")
                .field("version", MereNativeFieldSpec::U64, true)
                .field("address", MereNativeFieldSpec::String, true)
                .field("host", MereNativeFieldSpec::String, false)
                .build(),
        );
        validator.register(
            FacetId::new(NOTE_DOCUMENT_FACET),
            MereNativeSchemaBuilder::new("turnstone.note/v1")
                .description("Authored note profile")
                .field("version", MereNativeFieldSpec::U64, true)
                .field("format", MereNativeFieldSpec::String, true)
                .build(),
        );
        validator.register(
            FacetId::new(CLASS_FACET),
            SchemaDefinition {
                format: SchemaFormat::JsonSchema,
                schema_id: "chartulary.class/v1".to_string(),
                body: json!({"type": "string", "minLength": 1}),
            },
        );

        let mut registry = ClassRegistry::new();
        registry.register(
            ContentClass::new(
                WEB_PAGE_CLASS,
                [(
                    FacetId::new(WEB_PAGE_FACET),
                    "turnstone.web-page/v1".to_string(),
                )],
            )
            .with_label("Web page"),
        );
        registry.register(
            ContentClass::new(
                NOTE_CLASS,
                [(
                    FacetId::new(NOTE_DOCUMENT_FACET),
                    "turnstone.note/v1".to_string(),
                )],
            )
            .with_label("Note"),
        );
        Self {
            registry,
            validator,
        }
    }
}

enum BuiltinClass {
    WebPage,
    Note,
}

fn classify(node: &mere::kernel::graph::Node) -> Option<BuiltinClass> {
    let url = node.url();
    if node.body.is_some() || url.starts_with("mere://") {
        Some(BuiltinClass::Note)
    } else if url.starts_with("http://") || url.starts_with("https://") {
        Some(BuiltinClass::WebPage)
    } else {
        None
    }
}

/// Reconcile Turnstone's built-in class declarations and required profile
/// facets against the live graph. Returns changed nodes.
pub(crate) fn reconcile(canvas: &mut mere::canvas::Canvas) -> Result<usize, FacetError> {
    let builtins = BuiltinContentClasses::new();
    let class_facet = FacetId::new(CLASS_FACET);
    let web_facet = FacetId::new(WEB_PAGE_FACET);
    let note_facet = FacetId::new(NOTE_DOCUMENT_FACET);
    let mut changed = 0;

    let assignments = canvas
        .graph()
        .nodes()
        .filter_map(|(_, node)| {
            classify(node).map(|class| {
                (
                    node.id,
                    class,
                    node.url().to_string(),
                    node.media_type.clone(),
                )
            })
        })
        .collect::<Vec<_>>();
    let facets = canvas.facets_mut();
    for (node_id, class, address, media_type) in assignments {
        let before = facets.facets_of(&node_id).cloned();
        match class {
            BuiltinClass::WebPage => {
                facets.remove(&node_id, &note_facet);
                let host = url::Url::parse(&address)
                    .ok()
                    .and_then(|parsed| parsed.host_str().map(str::to_owned))
                    .unwrap_or_default();
                facets.set(
                    node_id,
                    web_facet.clone(),
                    json!({"version": 1, "address": address, "host": host}),
                    &builtins.validator,
                )?;
                facets.set(
                    node_id,
                    class_facet.clone(),
                    json!(WEB_PAGE_CLASS),
                    &builtins.validator,
                )?;
            }
            BuiltinClass::Note => {
                facets.remove(&node_id, &web_facet);
                facets.set(
                    node_id,
                    note_facet.clone(),
                    json!({
                        "version": 1,
                        "format": media_type.as_deref().unwrap_or("text/djot")
                    }),
                    &builtins.validator,
                )?;
                facets.set(
                    node_id,
                    class_facet.clone(),
                    json!(NOTE_CLASS),
                    &builtins.validator,
                )?;
            }
        }
        let node_facets = facets
            .facets_of(&node_id)
            .expect("class reconciliation just wrote node facets");
        let ClassMembership::Known(class) = builtins.registry.membership(node_facets) else {
            return Err(FacetError {
                facet: class_facet.clone(),
                reason: "reconciled content class is not registered".into(),
            });
        };
        class
            .admits(node_facets, &builtins.validator)
            .map_err(|error| match error {
                ClassError::MissingFacet(facet) => FacetError {
                    facet,
                    reason: "required content-class facet is absent".into(),
                },
                ClassError::InvalidFacet(error) => error,
            })?;
        if facets.facets_of(&node_id) != before.as_ref() {
            changed += 1;
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chartulary::ClassMembership;

    #[test]
    fn web_and_note_classes_coexist_and_are_admitted_in_one_graph() {
        let mut canvas = mere::canvas::Canvas::new();
        let web = canvas.visit("https://example.com/");
        let note = canvas.visit("mere://field-notes");
        let note_id = canvas.graph().get_node(note).unwrap().id;
        assert_eq!(reconcile(&mut canvas).unwrap(), 2);
        let builtins = BuiltinContentClasses::new();
        for key in [web, note] {
            let node = canvas.graph().get_node(key).unwrap();
            let node_facets = canvas.facets().facets_of(&node.id).unwrap();
            let ClassMembership::Known(class) = builtins.registry.membership(node_facets) else {
                panic!("built-in class must resolve");
            };
            class.admits(node_facets, &builtins.validator).unwrap();
        }
        assert_eq!(
            canvas.facets().get(
                &canvas.graph().get_node(web).unwrap().id,
                &FacetId::new(CLASS_FACET),
            ),
            Some(&json!(WEB_PAGE_CLASS))
        );
        assert_eq!(
            canvas.facets().get(&note_id, &FacetId::new(CLASS_FACET)),
            Some(&json!(NOTE_CLASS))
        );
    }

    #[test]
    fn reconciliation_is_idempotent() {
        let mut canvas = mere::canvas::Canvas::new();
        canvas.visit("https://example.com/");
        assert_eq!(reconcile(&mut canvas).unwrap(), 1);
        assert_eq!(reconcile(&mut canvas).unwrap(), 0);
    }
}
