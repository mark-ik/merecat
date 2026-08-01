//! Headless App constructors for tests and the projection host: an app with
//! no ports and a scratch data root, so a test drives the spine without a
//! window or a profile.

use std::path::PathBuf;

use mere::canvas::Canvas;

use crate::content::ContentStates;
use crate::panes::FrisketLayout;
use crate::surface::FocusTarget;
use crate::ui::OmnibarState;

use super::App;

impl App {
    fn isolated(data_root: PathBuf) -> Self {
        let identity =
            crate::identity::load_or_create_root(&data_root, &data_root.join("personae-vault"));
        let root = identity::IdentityProvider::master_public_key(identity.as_ref()).to_bytes();
        Self {
            canvas: Canvas::new(),
            omnibar: OmnibarState::default(),
            data_root,
            sessions: session_runtime::ManifestStore::new(),
            session_id: crate::panes::SessionId::new(),
            content: ContentStates::default(),
            place: crate::place::PlaceState::default(),
            next_place_generation: 0,
            next_place_request: 0,
            focus: FocusTarget::Canvas,
            frisket: FrisketLayout::default(),
            history: chrome::nav::History::new(""),
            active_pane: None,
            workbench: mere::platen::Workbench::new(),
            browser: session_runtime::browser_node_state::BrowserNodeStates::new(),
            physics_damping: session_runtime::DEFAULT_PHYSICS_DAMPING,
            maximized: None,
            window_count: 1,
            lenses: Vec::new(),
            roster_tab: 0,
            removed: Vec::new(),
            trash: Vec::new(),
            pending_install: None,
            denizens: crate::denizen::Denizens::new(root),
            identity,
            journal: {
                let (journal, hook) = mere::kernel::graph::journal_capture_hook();
                mere::kernel::graph::set_captured_delta_hook(Some(hook));
                journal
            },
            next_pane_id: 1,
            events: Vec::new(),
        }
    }

    /// Deterministic live graph truth for Graphshell's headed G3 receipt.
    pub(crate) fn projection_fixture() -> Self {
        use mere::kernel::geometry::PortablePoint;
        use mere::kernel::graph::apply::{add_node, assert_relation};
        use mere::kernel::graph::{EdgeAssertion, Graph, SemanticSubKind};

        let mut app = Self::isolated(std::env::temp_dir().join("turnstone-graphshell-g3"));
        let mut graph = Graph::new();
        let notes = add_node(
            &mut graph,
            Some(uuid::Uuid::from_u128(0x101)),
            "mere://field-notes".to_string(),
            PortablePoint::zero(),
        );
        let radios = add_node(
            &mut graph,
            Some(uuid::Uuid::from_u128(0x102)),
            "mere://radio-map".to_string(),
            PortablePoint::zero(),
        );
        let harmony = add_node(
            &mut graph,
            Some(uuid::Uuid::from_u128(0x103)),
            "mere://harmony-map".to_string(),
            PortablePoint::zero(),
        );
        let relation = || EdgeAssertion::Semantic {
            sub_kind: SemanticSubKind::Hyperlink,
            label: None,
            decay_progress: None,
        };
        let _ = assert_relation(&mut graph, notes, radios, relation());
        let _ = assert_relation(&mut graph, notes, harmony, relation());
        app.canvas.set_graph(graph);
        let _ = app
            .canvas
            .set_node_title_for(uuid::Uuid::from_u128(0x101), "Field notes".into());
        let _ = app
            .canvas
            .set_node_title_for(uuid::Uuid::from_u128(0x102), "Radio map".into());
        let _ = app
            .canvas
            .set_node_title_for(uuid::Uuid::from_u128(0x103), "Harmony map".into());
        app
    }

    #[cfg(test)]
    pub(crate) fn test_stub() -> Self {
        Self::isolated(std::env::temp_dir().join("turnstone-app-test"))
    }
}
