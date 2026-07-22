//! Merecat's Graphshell endpoint.
//!
//! The adapter reads Merecat's live graph, asks Mere cartography to disclose a
//! product-free score and scene, and keeps all rendered card data beside that
//! scene. Incoming intents return through Merecat's ordinary [`Action`] spine
//! only after Servitor has evaluated the endpoint's projected grant.

use std::collections::{BTreeMap, HashMap};

use chartulary::{Container, EditSpec, GraphLog, Relation};
use graphshell_client::{
    ClientState, PresentationResolution, ResolutionError, ResolvedPresentation,
};
use graphshell_endpoint::{IntentSink, PresentationSource, ProjectionSource};
use graphshell_protocol::{
    AdvertisedAction, BoundsRelationship, CachePolicy, CapabilityProfile, CardValueV1, ContentHash,
    IntentEffect, IntentInvocation, IntentReference, IntentResult, NativeGlyphV1, PortableCardV1,
    PresentationBinding, PresentationCapability, PresentationCodec, PresentationKey,
    PresentationManifest, PresentationOffer, PresentationSemantics, ProjectionRequest,
    ProjectionSession, ProjectionSnapshot, ProtocolVersion, ResourceRequest, ResourceResponse,
    SemanticRole,
};
use mere::kernel::graph::NodeKey;
use sceno::{Arrangement, Score, Spiral};
use scenotime::{Revision, SceneEpoch, SceneSnapshot};
use servitor::{Gate, Grant, Mode, PrefixAuthority, Subject};

use crate::action::Action;
use crate::app::App;

const SESSION: &str = "loopback:merecat:g3";
const LAYOUT_SCOPE: &str = "projection/layout/";
const GRAPH_SCOPE: &str = "graph/open/";
const FIT_INTENT: &str = "merecat.fit-view";
const OPEN_INTENT: &str = "merecat.open-address";

/// The product endpoint. Its default card extent can be replaced by the host
/// once a real retained card has measured itself.
pub struct MerecatEndpoint {
    app: App,
    session: ProjectionSession,
    card_extent: (f32, f32),
    snapshot: Option<ProjectionSnapshot>,
    resources: BTreeMap<ContentHash, Vec<u8>>,
    gate: Gate,
    authority: PrefixAuthority,
    audit: GraphLog<Container, Relation>,
    subject: Subject,
}

impl MerecatEndpoint {
    pub fn new(app: App) -> Result<Self, String> {
        Self::with_card_extent(app, (248.0, 168.0))
    }

    pub fn with_card_extent(app: App, card_extent: (f32, f32)) -> Result<Self, String> {
        let session = ProjectionSession(SESSION.into());
        let subject = Subject::new(*blake3::hash(session.0.as_bytes()).as_bytes());
        let gate = Gate::new();
        let mut audit = GraphLog::new();
        gate.project_grant(&mut audit, &Grant::new(subject, LAYOUT_SCOPE, Mode::Write))
            .map_err(|error| format!("failed to project endpoint grant: {error:?}"))?;

        // As in the resident-denizen lane, authority is rebuilt from the
        // gate-authored projection. The table is a read index, not another
        // source of grants.
        let mut authority = PrefixAuthority::new();
        for (_, node) in audit.graph().nodes() {
            if let Some(path) = node.id.strip_prefix(servitor::GRANT_PREFIX) {
                authority.grant(Grant::new(subject, path, Mode::Write));
            }
        }

        Ok(Self {
            app,
            session,
            card_extent,
            snapshot: None,
            resources: BTreeMap::new(),
            gate,
            authority,
            audit,
            subject,
        })
    }

    pub fn session(&self) -> &ProjectionSession {
        &self.session
    }

    pub fn app(&self) -> &App {
        &self.app
    }

    pub fn audit(&self) -> &GraphLog<Container, Relation> {
        &self.audit
    }

    fn actions() -> Vec<AdvertisedAction> {
        vec![
            AdvertisedAction {
                intent: IntentReference(FIT_INTENT.into()),
                label: "Fit view".into(),
                explanation: "Frame the disclosed Merecat graph without changing it.".into(),
                payload_schema: r#"{"type":"null"}"#.into(),
                effect: IntentEffect::Curation,
            },
            AdvertisedAction {
                intent: IntentReference(OPEN_INTENT.into()),
                label: "Open address".into(),
                explanation: "Ask Merecat to add or select an address in graph truth.".into(),
                payload_schema: r#"{"type":"string","format":"uri"}"#.into(),
                effect: IntentEffect::DomainTruth,
            },
        ]
    }

    fn presentation(&mut self, scene: &SceneSnapshot) -> Result<PresentationManifest, String> {
        let mut manifest = PresentationManifest::default();
        let graph = self.app.canvas.graph();
        for (instance, item) in scene.active_items_in_order() {
            let source = scene
                .tables
                .sources
                .get(item.source.0 as usize)
                .and_then(Option::as_ref)
                .ok_or_else(|| format!("scene item {} has no source", instance.0))?;
            let id = uuid::Uuid::parse_str(&source.id)
                .map_err(|error| format!("invalid Mere node id {}: {error}", source.id))?;
            let (key, node) = graph
                .get_node_by_id(id)
                .ok_or_else(|| format!("Mere node {id} vanished during disclosure"))?;
            let label = graph.node_display_label(key);
            let semantics = PresentationSemantics {
                label: label.clone(),
                role: SemanticRole::Article,
                bounds: BoundsRelationship::FillFootprint,
                actions: Self::actions(),
            };
            let card = PortableCardV1 {
                title: label.clone(),
                values: vec![
                    CardValueV1 {
                        label: "Address".into(),
                        value: node.url().to_string(),
                    },
                    CardValueV1 {
                        label: "Source".into(),
                        value: "Merecat graph".into(),
                    },
                ],
                badges: vec!["merecat".into(), "granted projection".into()],
                media: Vec::new(),
            };
            let glyph = NativeGlyphV1 {
                label,
                icon: Some("◇".into()),
                color: Some("#d8a657".into()),
            };
            let card_bytes = serde_json::to_vec(&card)
                .map_err(|error| format!("could not encode card: {error}"))?;
            let glyph_bytes = serde_json::to_vec(&glyph)
                .map_err(|error| format!("could not encode glyph: {error}"))?;
            let card_hash = ContentHash::of(&card_bytes);
            let glyph_hash = ContentHash::of(&glyph_bytes);
            self.resources.insert(card_hash, card_bytes.clone());
            self.resources.insert(glyph_hash, glyph_bytes.clone());
            let key = PresentationKey(source.id.clone());
            manifest.bindings.push(PresentationBinding {
                instance,
                key: key.clone(),
            });
            manifest.offers.insert(
                key,
                vec![
                    PresentationOffer {
                        codec: PresentationCodec::PortableCardV1,
                        resource: card_hash,
                        byte_size: card_bytes.len() as u64,
                        requires: PresentationCapability::PortableCard,
                        semantics: semantics.clone(),
                    },
                    PresentationOffer {
                        codec: PresentationCodec::NativeGlyphV1,
                        resource: glyph_hash,
                        byte_size: glyph_bytes.len() as u64,
                        requires: PresentationCapability::NativeGlyph,
                        semantics,
                    },
                ],
            );
        }
        Ok(manifest)
    }

    fn active_revision(&self) -> Option<(SceneEpoch, Revision)> {
        self.snapshot
            .as_ref()
            .map(|snapshot| (snapshot.scene.epoch, snapshot.scene.revision))
    }

    fn intent_was_advertised(&self, intent: &IntentInvocation) -> bool {
        self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.scene.active_item(intent.target).is_some()
                && snapshot
                    .presentation
                    .offers_for(intent.target)
                    .is_some_and(|offers| {
                        offers.iter().any(|offer| {
                            offer
                                .semantics
                                .actions
                                .iter()
                                .any(|action| action.intent.0 == intent.intent)
                        })
                    })
        })
    }

    fn petition(&mut self, scope: &str, node: Container) -> Result<(), servitor::GateError> {
        let revision = self.audit.revision();
        self.gate.petition(
            &self.authority,
            &mut self.audit,
            self.subject,
            scope,
            revision,
            vec![EditSpec::InsertNode(node)],
        )?;
        Ok(())
    }
}

impl ProjectionSource for MerecatEndpoint {
    type Error = String;

    fn snapshot(&mut self, request: ProjectionRequest) -> Result<ProjectionSnapshot, Self::Error> {
        if request.session != self.session {
            return Err("projection request names the wrong session".into());
        }
        if request.version.major != ProtocolVersion::V1.major {
            return Err("projection request uses an unsupported protocol".into());
        }
        if request.score.version != sceno::SCORE_VERSION {
            return Err("projection request uses an unsupported score".into());
        }
        let Arrangement::Spiral(spiral) = request.score.arrangement else {
            return Err("G3 Merecat endpoint currently accepts Spiral arrangements".into());
        };

        let graph = self.app.canvas.graph();
        let extents: HashMap<NodeKey, (f32, f32)> = graph
            .nodes()
            .map(|(key, _)| (key, self.card_extent))
            .collect();
        let mut mapped = cartography::project_spiral_score(
            graph,
            Some(&extents),
            self.app.canvas.focused_key(),
            true,
        );
        mapped.score.arrangement = Arrangement::Spiral(spiral);
        let solved = scenomise::solve(&mapped.score);
        let mut scene = cartography::scene_from_projection(
            &mapped.projection,
            |key| {
                graph
                    .get_node(key)
                    .expect("projection key came from the live graph")
                    .id
                    .to_string()
            },
            |key| extents.get(&key).copied(),
        );
        for ((item, score_item), solved_item) in scene
            .items
            .iter_mut()
            .zip(mapped.score.items.iter())
            .zip(solved.items.iter())
        {
            item.transform = solved_item.transform;
            item.representation = score_item.representation.clone();
            item.layer = score_item.layer;
            item.visible = score_item.visible;
        }
        for relation in &mut scene.relations {
            let from = scene.items[relation.from.0 as usize].transform.translate;
            let to = scene.items[relation.to.0 as usize].transform.translate;
            relation.points = vec![from, to];
        }
        scene.bounds = solved.bounds;
        scene.generation = mapped.score.generation;

        let revision = Revision(graph.revision().max(1));
        let scene = SceneSnapshot::from_dense(SceneEpoch(1), revision, scene)
            .map_err(|error| format!("invalid disclosed scene: {error:?}"))?;
        self.resources.clear();
        let presentation = self.presentation(&scene)?;
        let snapshot = ProjectionSnapshot {
            version: ProtocolVersion::V1,
            session: self.session.clone(),
            scene,
            presentation,
            cache_policy: CachePolicy::default(),
        };
        self.snapshot = Some(snapshot.clone());
        Ok(snapshot)
    }
}

impl PresentationSource for MerecatEndpoint {
    type Error = String;

    fn resource(&mut self, request: ResourceRequest) -> Result<ResourceResponse, Self::Error> {
        if request.session != self.session {
            return Err("resource request names the wrong session".into());
        }
        let bytes = self
            .resources
            .get(&request.resource)
            .cloned()
            .ok_or_else(|| "resource was not disclosed by this session".to_string())?;
        Ok(ResourceResponse {
            session: self.session.clone(),
            resource: request.resource,
            bytes,
        })
    }
}

impl IntentSink for MerecatEndpoint {
    type Error = String;

    fn invoke(&mut self, intent: IntentInvocation) -> Result<IntentResult, Self::Error> {
        if intent.session != self.session {
            return Err("intent names the wrong session".into());
        }
        let Some((epoch, revision)) = self.active_revision() else {
            return Err("intent arrived before a snapshot".into());
        };
        if intent.observed_epoch != epoch || intent.observed_revision != revision {
            return Ok(IntentResult::Stale {
                current_epoch: epoch,
                current_revision: revision,
            });
        }
        if !self.intent_was_advertised(&intent) {
            return Ok(IntentResult::Rejected {
                reason: "target or intent was not disclosed by this snapshot".into(),
            });
        }

        match intent.intent.as_str() {
            FIT_INTENT => {
                if !intent.payload.is_empty() {
                    return Ok(IntentResult::Rejected {
                        reason: "fit-view accepts an empty payload".into(),
                    });
                }
                let audit = Container::new(format!("{LAYOUT_SCOPE}fit-{}", self.audit.revision()))
                    .with_title("Fit disclosed Merecat graph");
                match self.petition(LAYOUT_SCOPE, audit) {
                    Ok(()) => {
                        self.app.update(Action::FitView);
                        Ok(IntentResult::Accepted)
                    }
                    Err(error) => Ok(IntentResult::Rejected {
                        reason: format!("authority gate refused fit-view: {error:?}"),
                    }),
                }
            }
            OPEN_INTENT => {
                let Ok(address) = String::from_utf8(intent.payload) else {
                    return Ok(IntentResult::Rejected {
                        reason: "open-address payload is not UTF-8".into(),
                    });
                };
                if address.trim().is_empty() {
                    return Ok(IntentResult::Rejected {
                        reason: "open-address requires an address".into(),
                    });
                }
                let audit = Container::new(format!("{GRAPH_SCOPE}open-{}", self.audit.revision()))
                    .with_title(format!("Open {address}"));
                match self.petition(GRAPH_SCOPE, audit) {
                    Ok(()) => {
                        if let Ok(mut journal) = self.app.journal.lock() {
                            journal.set_author(self.subject.to_hex());
                        }
                        self.app.update(Action::OpenAddress(address));
                        if let Ok(mut journal) = self.app.journal.lock() {
                            journal.set_author(mere::kernel::graph::USER_AUTHOR);
                        }
                        Ok(IntentResult::Accepted)
                    }
                    Err(error) => Ok(IntentResult::Rejected {
                        reason: format!("authority gate refused graph change: {error:?}"),
                    }),
                }
            }
            _ => Ok(IntentResult::Rejected {
                reason: "unknown endpoint intent".into(),
            }),
        }
    }
}

/// Material returned by the executable G3 canary and its receipt tests.
pub struct G3Run {
    pub session: ProjectionSession,
    pub presentations: Vec<ResolvedPresentation>,
    pub layout: graphshell::view::ProjectionLayoutView,
    pub fit_result: IntentResult,
    pub open_result: IntentResult,
    pub graph_revision_before: u64,
    pub graph_revision_after: u64,
    pub graph_nodes_before: usize,
    pub graph_nodes_after: usize,
    pub audit_revision: u64,
    pub audit_author: String,
}

pub fn run_g3_canary() -> Result<G3Run, String> {
    let mut endpoint = MerecatEndpoint::new(App::projection_fixture())?;
    let session = endpoint.session().clone();
    let request = ProjectionRequest {
        version: ProtocolVersion::V1,
        session: session.clone(),
        score: Score::new(Arrangement::Spiral(Spiral::default())),
    };
    let snapshot = endpoint.snapshot(request)?;
    let layout = graphshell::view::ProjectionLayoutView::from_scene(&snapshot.scene);
    let graph_revision_before = endpoint.app().canvas.graph().revision();
    let graph_nodes_before = endpoint.app().canvas.graph().nodes().count();
    let mut client = ClientState::default();
    client
        .apply_snapshot(snapshot)
        .map_err(|error| format!("client rejected Merecat snapshot: {error:?}"))?;
    let profile = CapabilityProfile::new([
        PresentationCapability::PortableCard,
        PresentationCapability::NativeGlyph,
    ]);
    let presentations = resolve_all(&mut endpoint, &mut client, &session, &profile)?;
    let ack = client
        .acknowledgement(&session)
        .ok_or_else(|| "client did not acknowledge the Merecat snapshot".to_string())?;
    let target = client
        .mounted(&session)
        .and_then(|mounted| {
            mounted
                .scene
                .active_items_in_order()
                .first()
                .map(|(id, _)| *id)
        })
        .ok_or_else(|| "Merecat snapshot disclosed no target".to_string())?;
    let fit_result = endpoint.invoke(IntentInvocation {
        session: session.clone(),
        target,
        observed_epoch: ack.epoch,
        observed_revision: ack.revision,
        intent: FIT_INTENT.into(),
        payload: Vec::new(),
    })?;
    let open_result = endpoint.invoke(IntentInvocation {
        session: session.clone(),
        target,
        observed_epoch: ack.epoch,
        observed_revision: ack.revision,
        intent: OPEN_INTENT.into(),
        payload: b"mere://graphshell/rejected".to_vec(),
    })?;
    let graph_revision_after = endpoint.app().canvas.graph().revision();
    let graph_nodes_after = endpoint.app().canvas.graph().nodes().count();
    let audit_revision = endpoint.audit().revision();
    let audit_author = endpoint
        .audit()
        .log()
        .entries()
        .last()
        .map(|entry| entry.author.as_str().to_string())
        .unwrap_or_default();

    Ok(G3Run {
        session,
        presentations,
        layout,
        fit_result,
        open_result,
        graph_revision_before,
        graph_revision_after,
        graph_nodes_before,
        graph_nodes_after,
        audit_revision,
        audit_author,
    })
}

fn resolve_all(
    endpoint: &mut MerecatEndpoint,
    client: &mut ClientState,
    session: &ProjectionSession,
    profile: &CapabilityProfile,
) -> Result<Vec<ResolvedPresentation>, String> {
    let instances: Vec<_> = client
        .mounted(session)
        .ok_or_else(|| "client did not mount the Merecat projection".to_string())?
        .scene
        .active_items_in_order()
        .into_iter()
        .map(|(instance, _)| instance)
        .collect();
    let mut resolved = Vec::new();
    for instance in instances {
        let presentation = match client
            .resolve(session, instance, profile)
            .map_err(resolution_error)?
        {
            PresentationResolution::Ready(presentation) => presentation,
            PresentationResolution::NeedsResource(request) => {
                let response = endpoint.resource(request)?;
                client
                    .apply_resource(response)
                    .map_err(|error| format!("client refused Merecat resource: {error:?}"))?;
                match client
                    .resolve(session, instance, profile)
                    .map_err(resolution_error)?
                {
                    PresentationResolution::Ready(presentation) => presentation,
                    PresentationResolution::NeedsResource(_) => {
                        return Err("Merecat resource remained unresolved after transfer".into());
                    }
                }
            }
        };
        resolved.push(presentation);
    }
    Ok(resolved)
}

fn resolution_error(error: ResolutionError) -> String {
    format!("could not resolve Merecat presentation: {error:?}")
}

pub fn render_g3_receipt() -> Result<String, String> {
    let run = run_g3_canary()?;
    let result_name = |result: &IntentResult| match result {
        IntentResult::Accepted => "Accepted".to_string(),
        IntentResult::Rejected { .. } => "Rejected".to_string(),
        IntentResult::Stale { .. } => "Stale".to_string(),
    };
    let result_detail = |result: &IntentResult, accepted: &str| match result {
        IntentResult::Accepted => accepted.to_string(),
        IntentResult::Rejected { reason } => reason.clone(),
        IntentResult::Stale { .. } => "The client observation was stale.".into(),
    };
    Ok(graphshell::view::render_projection_receipt(
        &graphshell::view::ProjectionReceiptView {
            eyebrow: "Graphshell · G3 receipt".into(),
            title: "Merecat truth, projected.".into(),
            lede: "Mere cartography maps the live browser graph into one Scenograph scene. Graphshell resolves the endpoint-owned cards and returns both intents through the same Servitor authority gate.".into(),
            session: run.session.0,
            status: format!(
                "Live · {} nodes · graph revision {}",
                run.graph_nodes_after, run.graph_revision_after
            ),
            presentations: run.presentations,
            layout: Some(run.layout),
            intents: vec![
                graphshell::view::IntentReceiptView {
                    label: "Fit view · curation".into(),
                    result: result_name(&run.fit_result),
                    detail: result_detail(
                        &run.fit_result,
                        "The projected layout grant admitted the harmless view action.",
                    ),
                },
                graphshell::view::IntentReceiptView {
                    label: "Open address · graph truth".into(),
                    result: result_name(&run.open_result),
                    detail: result_detail(
                        &run.open_result,
                        "The graph-changing action was admitted and lowered through App::update.",
                    ),
                },
            ],
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_mere_graph_becomes_cards_and_routed_relations() {
        let mut endpoint = MerecatEndpoint::new(App::projection_fixture()).unwrap();
        let snapshot = endpoint
            .snapshot(ProjectionRequest {
                version: ProtocolVersion::V1,
                session: endpoint.session().clone(),
                score: Score::new(Arrangement::Spiral(Spiral::default())),
            })
            .unwrap();
        assert_eq!(snapshot.scene.active_item_count(), 3);
        assert_eq!(snapshot.scene.tables.relations.iter().flatten().count(), 2);
        assert!(
            snapshot
                .scene
                .tables
                .sources
                .iter()
                .flatten()
                .all(|source| source.adapter == cartography::MERE_GRAPH_ADAPTER)
        );
        assert_eq!(snapshot.presentation.bindings.len(), 3);
    }

    #[test]
    fn projected_grant_accepts_view_and_refuses_graph_change() {
        let run = run_g3_canary().unwrap();
        assert_eq!(run.fit_result, IntentResult::Accepted);
        assert!(matches!(run.open_result, IntentResult::Rejected { .. }));
        assert_eq!(run.graph_nodes_before, run.graph_nodes_after);
        assert_eq!(run.graph_revision_before, run.graph_revision_after);
        assert_eq!(run.audit_revision, 2, "grant plus accepted petition");
        assert!(run.audit_author.starts_with("denizen:"));
        assert_eq!(run.layout.placements.len(), 3);
        assert_eq!(run.layout.relations.len(), 2);
    }

    #[test]
    fn committed_receipt_matches_the_live_merecat_endpoint() {
        assert_eq!(
            render_g3_receipt().unwrap(),
            include_str!("../docs/receipts/g3_merecat_endpoint.html")
        );
    }
}
