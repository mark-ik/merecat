//! App-spine tests: every one drives `Action` -> `update` -> `Effect` and
//! asserts on app truth, so they read as the spine's own specification.

use super::*;

#[test]
fn place_binding_survives_switch_and_restart_as_a_worker_open() {
    let root = std::env::temp_dir().join(format!("turnstone-place-adopt-{}", uuid::Uuid::new_v4()));
    let mut app = App::test_stub();
    app.data_root = root.clone();

    let personal = crate::panes::SessionId::new();
    std::fs::create_dir_all(session::session_dir(&root, personal)).unwrap();
    app.adopt_session(personal);
    assert_eq!(app.place, crate::place::PlaceState::Personal);
    assert!(
        !session::place_binding_path(&app.session_dir()).exists(),
        "adopting a personal session does not invent a place sidecar"
    );

    let target = crate::panes::SessionId::new();
    let sdir = session::session_dir(&root, target);
    let binding = crate::place::PlaceBindingV1::new(
        crate::place::PlaceId([0x11; 32]),
        crate::place::SharedContainerId([0x22; 32]),
        crate::place::ChatSpaceId([0x33; 32]),
        "commons",
    )
    .unwrap();
    session::save_place_binding(&sdir, &binding).unwrap();
    let key = app.canvas.visit("https://shared.example");
    let shared_node = app.canvas.graph().get_node(key).unwrap().id;
    session::save_session_graph(&sdir, app.canvas.graph());

    let effects = app.adopt_session(target);
    assert!(
        matches!(
            &app.place,
            crate::place::PlaceState::Opening {
                binding: actual,
                generation: 2,
            } if actual == &binding
        ),
        "the app exposes the worker open instead of presenting the cache as current"
    );
    assert!(
        effects.iter().any(|effect| matches!(
            effect,
            Effect::OpenPlace {
                session,
                generation: 2,
                binding: actual,
            } if *session == target && actual == &binding
        )),
        "adoption asks the shell-owned worker to materialize the retained domains"
    );
    assert!(
        app.canvas.graph().get_node_by_id(shared_node).is_some(),
        "the cached graph remains immediately available"
    );

    let mut reopened = App::test_stub();
    reopened.data_root = root.clone();
    let effects = reopened.adopt_session(target);
    assert!(
        matches!(
            &reopened.place,
            crate::place::PlaceState::Opening {
                binding: actual,
                generation: 1,
            } if actual == &binding
        ),
        "a fresh app state requests the same durable binding"
    );
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::OpenPlace {
            session,
            generation: 1,
            binding: actual,
        } if *session == target && actual == &binding
    )));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn stale_place_update_from_a_departed_session_is_ignored() {
    let root = std::env::temp_dir().join(format!("turnstone-place-stale-{}", uuid::Uuid::new_v4()));
    let mut app = App::test_stub();
    app.data_root = root.clone();
    let first = crate::panes::SessionId::new();
    let second = crate::panes::SessionId::new();
    let first_binding = crate::place::PlaceBindingV1::new(
        crate::place::PlaceId([0x11; 32]),
        crate::place::SharedContainerId([0x12; 32]),
        crate::place::ChatSpaceId([0x13; 32]),
        "first",
    )
    .unwrap();
    let second_binding = crate::place::PlaceBindingV1::new(
        crate::place::PlaceId([0x21; 32]),
        crate::place::SharedContainerId([0x22; 32]),
        crate::place::ChatSpaceId([0x23; 32]),
        "second",
    )
    .unwrap();
    session::save_place_binding(&session::session_dir(&root, first), &first_binding).unwrap();
    session::save_place_binding(&session::session_dir(&root, second), &second_binding).unwrap();

    app.adopt_session(first);
    app.adopt_session(second);
    assert_eq!(app.session_id, second);
    let stale = crate::place::OfflinePlaceSnapshot {
        graph: crate::place::GraphCache {
            nodes: 99,
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(
        app.apply_update(Update::PlaceOpened {
            session: first,
            generation: 1,
            result: Ok(stale),
        })
        .is_empty(),
        "a stale worker answer produces no follow-up effect"
    );
    assert!(
        matches!(
            &app.place,
            crate::place::PlaceState::Opening {
                binding,
                generation: 2,
            } if binding == &second_binding
        ),
        "the stale answer cannot replace the active session's opening"
    );

    let current = crate::place::OfflinePlaceSnapshot {
        graph: crate::place::GraphCache {
            nodes: 2,
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        app.apply_update(Update::PlaceOpened {
            session: second,
            generation: 2,
            result: Ok(current.clone()),
        }),
        vec![Effect::Redraw]
    );
    assert_eq!(
        app.place,
        crate::place::PlaceState::Offline {
            binding: second_binding,
            generation: 2,
            snapshot: current,
        }
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_refused_invitation_leaves_no_binding_in_app_state() {
    let mut app = App::test_stub();
    let invite = Box::new(crate::place::invite::PlaceInviteV1 {
        version: crate::place::invite::PLACE_INVITE_VERSION,
        binding: crate::place::PlaceBindingV1::new(
            crate::place::PlaceId([0x71; 32]),
            crate::place::SharedContainerId([0x72; 32]),
            crate::place::ChatSpaceId([0x73; 32]),
            "hall",
        )
        .unwrap(),
        founder: [0x74; 32],
        inviter: [0x75; 32],
        inviter_prekey: inline_artifact(b"prekey"),
        governance: inline_artifact(b"drop"),
        key_welcome: inline_artifact(b"control"),
        key_direct: inline_artifact(b"direct"),
        expected_epoch: [0x76; 32],
        membership_heads: vec![[0x77; 32]],
        not_after_ms: u64::MAX,
        rendezvous: Vec::new(),
    });

    let effects = app.join_place(invite);
    let generation = match &app.place {
        // While admission runs the app names a generation and nothing else.
        // The envelope names a place; that is not the same as belonging to one.
        crate::place::PlaceState::Joining { generation } => *generation,
        other => panic!("join must begin in Joining, got {other:?}"),
    };
    assert!(app.place.binding().is_none());
    assert!(matches!(
        effects.as_slice(),
        [Effect::JoinPlace { generation: g, .. }] if *g == generation
    ));

    assert_eq!(
        app.apply_update(Update::PlaceJoined {
            session: app.session_id,
            generation,
            result: Err("Gemot membership does not contain this Personae root".into()),
        }),
        vec![Effect::Redraw]
    );
    // Failed, not Degraded: a refusal means there is no place, so nothing may
    // carry a binding that admission never granted.
    assert!(matches!(app.place, crate::place::PlaceState::Failed { .. }));
    assert!(app.place.binding().is_none());
}

#[test]
fn a_stale_join_answer_cannot_bind_a_session() {
    let mut app = App::test_stub();
    let binding = crate::place::PlaceBindingV1::new(
        crate::place::PlaceId([0x81; 32]),
        crate::place::SharedContainerId([0x82; 32]),
        crate::place::ChatSpaceId([0x83; 32]),
        "hall",
    )
    .unwrap();
    app.place = crate::place::PlaceState::Joining { generation: 7 };

    assert!(
        app.apply_update(Update::PlaceJoined {
            session: app.session_id,
            generation: 6,
            result: Ok((binding, crate::place::OfflinePlaceSnapshot::default())),
        })
        .is_empty(),
        "an answer from an earlier join produces no effect"
    );
    assert!(matches!(
        app.place,
        crate::place::PlaceState::Joining { generation: 7 }
    ));
}

#[test]
fn shared_nodes_arrive_without_disturbing_the_local_canvas() {
    let mut app = App::test_stub();
    // The person's own work: a node they opened and selected.
    let mine = app.canvas.visit("https://mine.example");
    let mine_id = app.canvas.graph().get_node(mine).unwrap().id;
    let before = app.canvas.graph().nodes().count();

    let shared = crate::place::projection::SharedGraph {
        nodes: vec![
            crate::place::projection::SharedNode {
                id: "https://theirs.example".into(),
                address: "https://theirs.example".into(),
            },
            // Already present locally: reconciliation must not duplicate it.
            crate::place::projection::SharedNode {
                id: "https://mine.example".into(),
                address: "https://mine.example".into(),
            },
        ],
    };

    assert_eq!(app.reconcile_shared_graph(&shared), 1, "only the missing one");
    assert_eq!(app.canvas.graph().nodes().count(), before + 1);
    assert!(
        app.canvas
            .graph()
            .get_node_by_url("https://theirs.example")
            .is_some(),
        "the shared address arrived"
    );
    assert_eq!(
        app.canvas.selected_members(),
        vec![mine_id],
        "a background reconcile must not move the person's selection"
    );

    // Idempotent: converging again changes nothing, which is what lets this
    // run on every resync.
    assert_eq!(app.reconcile_shared_graph(&shared), 0);
    assert_eq!(app.canvas.graph().nodes().count(), before + 1);

    // A node leaving the place does not remove the person's copy of it.
    assert_eq!(
        app.reconcile_shared_graph(&crate::place::projection::SharedGraph::default()),
        0
    );
    assert_eq!(app.canvas.graph().nodes().count(), before + 1);
}

#[test]
fn a_shared_knot_address_needs_no_path_of_its_own() {
    // T4's Commons half. A Knot document is an addressed node in the shared
    // graph, so it rides the ordinary share path and arrives through the
    // ordinary reconcile. The plan forbids inventing OpenSharedKnot or
    // SaveSharedKnot, and this is the assertion that keeps that honest: if
    // Knot ever needs its own share path, this test is what breaks.
    let mut app = App::test_stub();
    let shared = crate::place::projection::SharedGraph {
        nodes: vec![
            crate::place::projection::SharedNode {
                id: "file:///vault/minutes.knot".into(),
                address: "file:///vault/minutes.knot".into(),
            },
            crate::place::projection::SharedNode {
                id: "https://example.test/page".into(),
                address: "https://example.test/page".into(),
            },
        ],
    };

    assert_eq!(app.reconcile_shared_graph(&shared), 2);
    let knot = app
        .canvas
        .graph()
        .get_node_by_url("file:///vault/minutes.knot");
    assert!(knot.is_some(), "the Knot address reconciled like any other");

    // And it is recognizable as Knot, so the existing OpenAddress route sends
    // it to the authoring engine without the place port routing anything.
    assert!(crate::knot_authoring::is_knot_address(
        "file:///vault/minutes.knot"
    ));
    assert!(!crate::knot_authoring::is_knot_address(
        "https://example.test/page"
    ));
}

fn inline_artifact(bytes: &[u8]) -> crate::place::invite::ArtifactRefV1 {
    crate::place::invite::ArtifactRefV1::Inline {
        media_type: "application/vnd.mere.place-artifact".into(),
        digest: proofs::Digest::blake3(bytes)
            .bytes
            .as_slice()
            .try_into()
            .unwrap(),
        bytes: bytes.to_vec(),
    }
}

#[test]
fn malformed_place_binding_is_visible_without_hiding_cached_graph() {
    let root =
        std::env::temp_dir().join(format!("turnstone-place-failure-{}", uuid::Uuid::new_v4()));
    let mut app = App::test_stub();
    app.data_root = root.clone();
    let target = crate::panes::SessionId::new();
    let sdir = session::session_dir(&root, target);
    std::fs::create_dir_all(&sdir).unwrap();
    std::fs::write(session::place_binding_path(&sdir), b"{ not valid JSON").unwrap();
    let key = app.canvas.visit("https://cached.example");
    let cached_node = app.canvas.graph().get_node(key).unwrap().id;
    session::save_session_graph(&sdir, app.canvas.graph());

    app.adopt_session(target);
    assert!(
        matches!(
            &app.place,
            crate::place::PlaceState::Failed { error }
                if error.contains("place sidecar JSON")
        ),
        "the malformed binding is product-visible: {:?}",
        app.place
    );
    assert!(
        app.canvas.graph().get_node_by_id(cached_node).is_some(),
        "a binding fault does not discard the cached graph"
    );
    assert!(
        session::place_binding_path(&sdir).exists(),
        "adoption preserves the malformed sidecar for repair"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// The layout round-trip through the facet store: a session saved as
/// graph.json + `arrangement.*` facets (per-node) + `scene.*` facets (on
/// the container id) re-adopts with each node back at its saved position
/// and size, and the scene's own settings (physics damping) restored — the
/// graph itself is position-free, so without the facets every node would
/// park at the origin and the scene would reset to defaults.
#[test]
fn adopt_session_restores_the_saved_canvas_layout_from_facets() {
    let mut app = App::test_stub();
    app.data_root =
        std::env::temp_dir().join(format!("turnstone-facet-adopt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&app.data_root);
    // A manifest so the session has a container id for its `scene.*` facets.
    let container = uuid::Uuid::from_u128(0xc0ffee);
    app.sessions
        .insert(session_runtime::GraphSessionManifest::new(
            app.session_id,
            crate::panes::GraphId::from_uuid(container),
        ));
    let sdir = app.session_dir();
    std::fs::create_dir_all(&sdir).unwrap();

    // A one-node session on disk: the graph, its per-node arrangement (a
    // position and a size override), and the scene's own damping setting.
    let key = app.canvas.visit("https://layout.example");
    let id = app.canvas.graph().get_node(key).unwrap().id;
    session::save_session_graph(&sdir, app.canvas.graph());
    let mut facets = session_runtime::NodeFacetStore::new();
    session_runtime::write_arrangement_positions(&mut facets, [(id, (444.0, -55.0))]);
    session_runtime::write_arrangement_sizes(&mut facets, [(id, 96.0)]);
    session_runtime::write_scene_facets(
        &mut facets,
        container,
        &session_runtime::SceneFacets {
            physics_damping: 5.5,
            ..session_runtime::SceneFacets::default()
        },
    );
    // Browser state rides the same store now (web.* facets): live content
    // was ON for this node, so the adopt must respawn it.
    let mut browser = session_runtime::browser_node_state::BrowserNodeStates::new();
    browser.entry(id).content_on = true;
    session_runtime::write_web_states(&mut facets, &browser);
    session::save_node_facets(&sdir, &facets);

    // Adopt (the boot/switch seam): the node comes back AND lands where
    // it was left.
    let effects = app.adopt_session(app.session_id);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::SpawnContent { node, .. } if *node == id)),
        "content-on read from the web.content facet respawns on adopt"
    );
    let (restored, _) = app
        .canvas
        .graph()
        .get_node_by_url("https://layout.example")
        .expect("the graph restored");
    let pos = app
        .canvas
        .node_position(restored)
        .expect("a restored position");
    assert!(
        (pos.x - 444.0).abs() < 1.0 && (pos.y + 55.0).abs() < 1.0,
        "the facet layout is applied, got {pos:?}"
    );
    let size = app.canvas.node_size(restored);
    assert!(
        (size - 96.0).abs() < 0.001,
        "the size override rode the facets too, got {size}"
    );
    assert!(
        (app.physics_damping - 5.5).abs() < 0.001,
        "the scene.physics_damping facet restored, got {}",
        app.physics_damping
    );
    let _ = std::fs::remove_dir_all(&app.data_root);
}

/// The B1 residency arc, headless: a staged install shows its ask, the
/// confirm mints the denizen (node + binding facet + gate-projected grant
/// in a persisted nested world), and the runtime rebuilds from durable
/// truth alone.
#[test]
fn denizen_installs_after_visible_review() {
    let mut app = App::test_stub();
    app.data_root =
        std::env::temp_dir().join(format!("turnstone-denizen-b1-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&app.data_root);
    std::fs::create_dir_all(app.session_dir()).unwrap();
    let pack = app.data_root.join("trail-keeper.lua");
    std::fs::write(&pack, "mere.open('mere://kept/note')").unwrap();

    app.update(Action::InstallDenizen { path: pack.display().to_string() });
    assert!(app.pending_install.is_some());
    let rows = app.denizen_actions();
    assert!(
        rows[0].0.contains("grants:") && rows[0].0.contains("(lua)"),
        "the ask is the first palette row: {rows:?}"
    );
    assert!(
        app.take_events().iter().any(|e| matches!(e, AppEvent::DenizenStaged(_))),
        "staging is observable"
    );

    app.update(Action::ConfirmInstallDenizen);
    assert!(app.pending_install.is_none());
    assert_eq!(app.denizens.residents.len(), 1);
    let (&member, resident) = app.denizens.residents.iter().next().unwrap();
    let binding = session_runtime::read_denizen_binding(app.canvas.facets(), member)
        .expect("the binding facet is durable truth");
    assert_eq!(binding.subject, resident.subject.to_hex());
    assert_eq!(binding.kind, session_runtime::DenizenKind::Scenario);
    assert!(binding.legacy_nested_log.is_empty(), "the facet is pure agency");
    let borne = app
        .canvas
        .graph()
        .get_node_key_by_id(member)
        .and_then(|key| app.canvas.graph().get_node(key))
        .and_then(|node| node.nested.clone())
        .expect("the node BEARS its world");
    assert_eq!(borne.as_str(), resident.subject.to_hex(), "structure on the node");
    assert!(
        resident
            .nested
            .graph()
            .key_of(&servitor::Gate::projection_id(&crate::denizen::world_cap()))
            .is_some(),
        "the grant projection is in the nested world"
    );
    assert!(
        crate::denizen::nested_log_path(&app.session_dir(), &resident.subject.to_hex())
            .exists(),
        "the nested log persisted at its birth"
    );

    let rebuilt = crate::denizen::rebuild(
        app.canvas.facets(),
        app.canvas.graph(),
        &app.session_dir(),
        app.identity.as_ref(),
    );
    assert_eq!(rebuilt.residents.len(), 1);
    assert!(rebuilt.legacy_heals.is_empty(), "a fresh install needs no heal");
    assert!(
        servitor::AuthorityProvider::covers(
            &rebuilt.authority,
            resident.subject,
            &crate::denizen::world_cap(),
            servitor::Mode::Write
        ),
        "authority derives from the projection, not from a second store"
    );
    let _ = std::fs::remove_dir_all(&app.data_root);
}

/// The gate refuses a denizen's petition outside its granted scope, and
/// commits an in-scope one attributed — the servitor pipeline live over a
/// resident's actual nested world.
#[test]
fn resident_petitions_run_through_the_gate() {
    let mut app = App::test_stub();
    app.data_root =
        std::env::temp_dir().join(format!("turnstone-denizen-gate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&app.data_root);
    std::fs::create_dir_all(app.session_dir()).unwrap();
    let pack = app.data_root.join("keeper.lua");
    std::fs::write(&pack, "mere.open('mere://kept/note')").unwrap();
    app.update(Action::InstallDenizen { path: pack.display().to_string() });
    app.update(Action::ConfirmInstallDenizen);
    let (&member, _) = app.denizens.residents.iter().next().unwrap();

    let subject = app.denizens.residents[&member].subject;
    let authority = app.denizens.authority.clone();
    let gate = app.denizens.gate.clone();
    let resident = app.denizens.residents.get_mut(&member).unwrap();

    let rev = resident.nested.revision();
    let committed = gate
        .petition(
            &authority,
            &mut resident.nested,
            subject,
            &servitor::ScopePath::parse(crate::denizen::SCENARIO_SCOPE).unwrap(),
            rev,
            vec![chartulary::EditSpec::InsertNode(chartulary::Container::new(
                "scenario/kept-note",
            ))],
        )
        .expect("an in-scope petition commits");
    let entry = &resident.nested.log().entries()[committed.batch.0 as usize];
    assert_eq!(entry.author, subject.to_author(), "attributed to the denizen");

    let rev = resident.nested.revision();
    let err = gate
        .petition(
            &authority,
            &mut resident.nested,
            subject,
            &servitor::ScopePath::parse("notes").unwrap(),
            rev,
            vec![chartulary::EditSpec::InsertNode(chartulary::Container::new(
                "notes/sneaky",
            ))],
        )
        .unwrap_err();
    assert!(
        matches!(err, servitor::GateError::Unauthorized { .. }),
        "an ungranted path refuses: {err:?}"
    );
    let _ = std::fs::remove_dir_all(&app.data_root);
}

/// With the piccolo runtime: a run lowers the body's Actions through the
/// spine, and the journal attributes the captured edits to the subject.
#[cfg(feature = "piccolo")]
#[test]
fn denizen_runs_attributed() {
    let mut app = App::test_stub();
    app.data_root =
        std::env::temp_dir().join(format!("turnstone-denizen-run-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&app.data_root);
    std::fs::create_dir_all(app.session_dir()).unwrap();
    let pack = app.data_root.join("keeper.lua");
    std::fs::write(&pack, "mere.open('mere://kept/note')").unwrap();
    app.update(Action::InstallDenizen { path: pack.display().to_string() });
    app.update(Action::ConfirmInstallDenizen);
    let (&member, resident) = app.denizens.residents.iter().next().unwrap();
    let hex = resident.subject.to_hex();

    app.update(Action::RunDenizen { member });
    assert!(
        app.canvas.graph().get_node_by_url("mere://kept/note").is_some(),
        "the body's Action landed through the spine"
    );
    let journal = app.journal.lock().unwrap();
    assert!(
        journal.entries().iter().any(|e| e.author == hex),
        "the captured edit reads back attributed to the subject"
    );
    assert_eq!(
        journal.author(),
        mere::kernel::graph::USER_AUTHOR,
        "the author scope restored after the run"
    );
    let _ = std::fs::remove_dir_all(&app.data_root);
}

/// The fork arm (G4-R R2): forking from a node mints a new session whose
/// manifest carries the parent back-reference, snapshots the connected
/// component (not the rest of the graph), carries the donor's per-node
/// character as facets through the copy's id remap plus the container's
/// scene settings, and opens by session-switch.
#[test]
fn fork_session_snapshots_the_component_with_its_facets() {
    let mut app = App::test_stub();
    app.data_root =
        std::env::temp_dir().join(format!("turnstone-fork-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&app.data_root);
    let donor_container = uuid::Uuid::from_u128(0xd0);
    app.sessions
        .insert(session_runtime::GraphSessionManifest::new(
            app.session_id,
            crate::panes::GraphId::from_uuid(donor_container),
        ));
    std::fs::create_dir_all(app.session_dir()).unwrap();

    // A two-node connected component plus a disconnected bystander.
    let a = app.canvas.visit("https://fork.example/a");
    let a_id = app.canvas.graph().get_node(a).unwrap().id;
    app.update(Action::OpenAddress("https://fork.example/b".to_string()));
    let _bystander = {
        let mut g = app.canvas.graph().clone();
        let k = mere::kernel::graph::apply::add_node(
            &mut g,
            Some(uuid::Uuid::from_u128(0x10e)),
            "https://lone.example".to_string(),
            Default::default(),
        );
        let id = g.get_node(k).unwrap().id;
        app.canvas.set_graph(g);
        id
    };
    // Donor character: live content on `a` (so web.content refreshes true
    // from live truth) and a scene damping.
    app.physics_damping = 4.75;
    app.apply_update(Update::ContentSpawned {
        node: a_id,
        facts: None,
    });

    let donor_session = app.session_id;
    let effects = app.update(Action::ForkNode { member: a_id });
    let Some(crate::action::Effect::SwitchSession { id: fork_id }) = effects
        .iter()
        .find(|e| matches!(e, crate::action::Effect::SwitchSession { .. }))
        .cloned()
    else {
        panic!("fork returns the switch effect: {effects:?}");
    };
    assert_ne!(fork_id, donor_session);
    let fork_manifest = app.sessions.get(fork_id).expect("fork manifest inserted");
    assert_eq!(
        fork_manifest.parent_session,
        Some(donor_session),
        "the weak parent back-reference"
    );
    assert_ne!(
        fork_manifest.root_graph_id,
        crate::panes::GraphId::from_uuid(donor_container),
        "the fork minted its own real GraphId"
    );

    // The persisted fork: the 2-node component (not the bystander), and
    // the carried facets keyed by the REMAPPED ids.
    let fork_dir = session::session_dir(&app.data_root, fork_id);
    let fork_graph = session::load_session_graph(&fork_dir).expect("fork graph persisted");
    assert_eq!(fork_graph.nodes().count(), 2, "the component, nothing else");
    let fork_facets = session::load_node_facets(&fork_dir).expect("fork facets persisted");
    let fork_a = fork_graph
        .nodes()
        .find(|(_, n)| n.url() == "https://fork.example/a")
        .map(|(_, n)| n.id)
        .expect("the seed's copy");
    assert_ne!(fork_a, a_id, "a fork copy is a new entity");
    assert!(
        !session_runtime::read_arrangement_positions(&fork_facets).is_empty(),
        "the donor layout rode the carry"
    );
    let web = session_runtime::read_web_states(&fork_facets);
    assert!(
        web.get(fork_a).is_some_and(|s| s.content_on),
        "web.content carried onto the remapped id"
    );
    let scene = session_runtime::read_scene_facets(
        &fork_facets,
        *fork_manifest.root_graph_id.as_uuid(),
    );
    assert!(
        (scene.physics_damping - 4.75).abs() < 0.001,
        "scene.* carried donor-container -> fork-container"
    );
    let _ = std::fs::remove_dir_all(&app.data_root);
}

/// The world-carry: forking from a denizen node re-bears its nested graph
/// on the fork's copy AND copies the world file into the fork's session
/// dir — donor and fork hold independent worlds thereafter (the kernel
/// copy alone would leave the fork's denizen un-resided).
#[test]
fn fork_carries_denizen_worlds_as_real_copies() {
    let mut app = App::test_stub();
    app.data_root =
        std::env::temp_dir().join(format!("turnstone-fork-world-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&app.data_root);
    std::fs::create_dir_all(app.session_dir()).unwrap();
    let pack = app.data_root.join("keeper.lua");
    std::fs::write(&pack, "mere.open('mere://kept/note')").unwrap();
    app.update(Action::InstallDenizen { path: pack.display().to_string() });
    app.update(Action::ConfirmInstallDenizen);
    let (member, world_id, donor_revision) = {
        let (&member, resident) = app.denizens.residents.iter().next().unwrap();
        (member, resident.subject.to_hex(), resident.nested.revision())
    };

    let effects = app.fork_session_from(member);
    let Some(crate::action::Effect::SwitchSession { id: fork_id }) = effects
        .iter()
        .find(|e| matches!(e, crate::action::Effect::SwitchSession { .. }))
        .cloned()
    else {
        panic!("fork returns the switch effect: {effects:?}");
    };
    let fork_dir = session::session_dir(&app.data_root, fork_id);
    let fork_graph = session::load_session_graph(&fork_dir).expect("fork graph persisted");
    let (_, fork_node) = fork_graph
        .nodes()
        .find(|(_, n)| n.nested.is_some())
        .expect("the fork's copy bears the world");
    assert_ne!(fork_node.id, member, "a fork copy is a new entity");
    assert_eq!(
        fork_node.nested.as_ref().map(|log| log.as_str()),
        Some(world_id.as_str()),
        "same world identity, re-borne on the copy"
    );
    assert!(
        crate::denizen::nested_log_path(&fork_dir, &world_id).is_file(),
        "the fork owns a real world file"
    );
    assert!(
        crate::denizen::nested_log_path(&app.session_dir(), &world_id).is_file(),
        "the donor keeps its own"
    );
    // The fork rebuilds a full resident from its OWN dir, no legacy heal.
    let fork_facets = session::load_node_facets(&fork_dir).expect("fork facets persisted");
    let rebuilt = crate::denizen::rebuild(
        &fork_facets,
        &fork_graph,
        &fork_dir,
        app.identity.as_ref(),
    );
    assert_eq!(rebuilt.residents.len(), 1, "the fork's denizen resides");
    assert!(rebuilt.legacy_heals.is_empty());
    assert_eq!(
        rebuilt.residents.values().next().unwrap().nested.revision(),
        donor_revision,
        "the carried world is the donor's world, bit-for-bit at fork time"
    );
    let _ = std::fs::remove_dir_all(&app.data_root);
}

/// Overmap O3: closing a session moves its whole directory to the manifest
/// trash (the derived removed-sessions record — no parallel bin), and
/// recovery moves it back with identity intact and switches to it.
#[test]
fn close_session_trashes_and_recover_restores_identity() {
    let mut app = App::test_stub();
    app.data_root =
        std::env::temp_dir().join(format!("turnstone-o3-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&app.data_root);
    // Two real sessions on disk (manifests bound to the root so trash ops
    // have a home), the second is current.
    app.sessions =
        session_runtime::ManifestStore::with_root(session::sessions_root(&app.data_root));
    let keeper = crate::panes::SessionId::new();
    let mut keeper_m = session_runtime::GraphSessionManifest::new(
        keeper,
        crate::panes::GraphId::from_uuid(uuid::Uuid::from_u128(0xa)),
    );
    keeper_m.display_name = Some("keeper".to_string());
    app.sessions.insert(keeper_m);
    let closing_id = crate::panes::SessionId::new();
    let mut closing_m = session_runtime::GraphSessionManifest::new(
        closing_id,
        crate::panes::GraphId::from_uuid(uuid::Uuid::from_u128(0xb)),
    );
    closing_m.display_name = Some("expedition".to_string());
    app.sessions.insert(closing_m);
    app.sessions.flush_dirty().unwrap();
    app.session_id = closing_id;

    // Close: the action defers the disk half to the shell-ordered effect
    // (bin release first); apply_trash is that effect's app half.
    let effects = app.update(Action::CloseSession);
    assert!(matches!(
        effects[..],
        [crate::action::Effect::TrashSession { closing, next }]
            if closing == closing_id && next == keeper
    ));
    assert!(app.apply_trash(closing_id));
    assert_eq!(app.trash.len(), 1);
    assert_eq!(app.trash[0].session_id, closing_id);
    assert_eq!(app.trash[0].display_name.as_deref(), Some("expedition"));
    assert!(
        app.sessions.get(closing_id).is_none(),
        "gone from the live set"
    );

    // Recover: the manifest re-lists with the SAME id + graph id, the
    // trash cache empties, and the switch adopts it.
    let effects = app.update(Action::RecoverSession(closing_id));
    assert!(matches!(
        effects[..],
        [crate::action::Effect::SwitchSession { id }] if id == closing_id
    ));
    assert!(app.trash.is_empty(), "the trash entry is consumed");
    let recovered = app.sessions.get(closing_id).expect("re-listed");
    assert_eq!(
        recovered.root_graph_id,
        crate::panes::GraphId::from_uuid(uuid::Uuid::from_u128(0xb)),
        "identity intact"
    );
    assert!(
        app.take_events()
            .iter()
            .any(|e| matches!(e, AppEvent::SessionRecovered(l) if l == "expedition")),
        "the recovery event carries the label"
    );
    let _ = std::fs::remove_dir_all(&app.data_root);
}

/// Committing a `>` registry row lowers the registry Action through the
/// same spine as everything else, and the palette closes.
#[test]
fn committing_an_action_row_runs_the_action_and_closes() {
    let mut app = App::test_stub();
    for action in [
        Action::OmnibarOpen { command: true },
        Action::OmnibarChar('i'),
        Action::OmnibarChar('s'),
        Action::OmnibarChar('o'),
    ] {
        app.update(action);
    }
    assert!(!app.canvas.is_isometric());
    let effects = app.update(Action::OmnibarCommit);
    assert!(app.canvas.is_isometric(), "the committed toggle ran");
    assert!(!app.omnibar.open, "the palette closed on commit");
    assert!(effects.contains(&Effect::Redraw));
}

/// The content flip lowers through the spine: focused node -> Requested +
/// SpawnContent; the port's honest failure folds back; a failed node
/// retries on the next flip.
#[test]
fn content_flip_lowers_and_fails_honestly() {
    use crate::content::NodeContent;
    let mut app = App::test_stub();
    assert!(
        app.update(Action::ToggleNodeContent).is_empty(),
        "no focus, no-op"
    );
    app.canvas.visit("https://example.com/page");
    let effects = app.update(Action::ToggleNodeContent);
    let Some(Effect::SpawnContent { node, url }) = effects
        .iter()
        .find(|e| matches!(e, Effect::SpawnContent { .. }))
        .cloned()
    else {
        panic!("flip on a focused node spawns: {effects:?}");
    };
    assert_eq!(url, "https://example.com/page");
    assert_eq!(app.content.get(node), Some(&NodeContent::Requested));
    assert!(
        !app.update(Action::ToggleNodeContent)
            .iter()
            .any(|e| matches!(e, Effect::SpawnContent { .. })),
        "flipping an in-flight node closes, never double-spawns"
    );
    app.content.note_requested(node);
    app.apply_update(Update::ContentFailed {
        node,
        error: "port not wired".into(),
    });
    assert!(
        matches!(app.content.get(node), Some(NodeContent::Failed(_))),
        "failure is a surfaced state"
    );
    assert!(
        app.update(Action::ToggleNodeContent)
            .iter()
            .any(|e| matches!(e, Effect::SpawnContent { .. })),
        "a failed node retries on the next flip"
    );
}

/// The tear-out leaf arm (rung 7 depth): the active pane's leaf leaves
/// the primary tree and joins a lens space — SAME pane id (the retained
/// runner never moves; identity is structural). No lens open spawns one.
#[test]
fn tear_out_moves_the_leaf_and_keeps_its_id() {
    let mut app = App::test_stub();
    app.update(Action::SummonPane(PaneKind::Roster));
    let roster_id = app
        .frisket
        .iter_leaves()
        .find(|(_, c, _)| matches!(c, PaneContent::Roster))
        .map(|(id, _, _)| id)
        .expect("summoned");
    let effects = app.update(Action::TearOutActivePane);
    // Departure: the primary tree no longer holds a Roster leaf.
    assert!(
        !app.frisket
            .iter_leaves()
            .any(|(_, c, _)| matches!(c, PaneContent::Roster)),
        "the roster left the primary tree"
    );
    // Arrival: a lens space spawned (no lens was open) and holds the SAME
    // pane id — the leaf moved, nothing was recreated.
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::OpenWindow { .. })),
        "tearing out with no lens spawns one: {effects:?}"
    );
    let lens = app.lenses[0].as_ref().expect("lens space seeded");
    let moved = lens
        .iter_leaves()
        .find(|(_, c, _)| matches!(c, PaneContent::Roster))
        .expect("the roster landed in the lens");
    assert_eq!(moved.0, roster_id, "same pane id across the move");
    // The moved pane STAYS active, so pane-anchored ops follow it: a
    // summon lands beside it IN THE LENS (the window as pane host).
    assert_eq!(
        app.active_pane,
        Some(roster_id),
        "the moved pane stays active"
    );
    app.update(Action::SummonPane(PaneKind::Trail));
    let lens = app.lenses[0].as_ref().unwrap();
    assert!(
        lens.iter_leaves()
            .any(|(_, c, _)| matches!(c, PaneContent::Trail)),
        "summon-beside followed the active pane into the lens"
    );
    assert!(
        !app.frisket
            .iter_leaves()
            .any(|(_, c, _)| matches!(c, PaneContent::Trail)),
        "the summoned trail is not in the primary tree"
    );
    // A PRIMARY pane tearing out reuses the open lens (no window spam).
    app.active_pane = None;
    app.update(Action::SummonPane(PaneKind::Gloss));
    let effects = app.update(Action::TearOutActivePane);
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::OpenWindow { .. })),
        "an open lens is reused"
    );
    let lens = app.lenses[0].as_ref().unwrap();
    assert!(
        lens.iter_leaves()
            .any(|(_, c, _)| matches!(c, PaneContent::Gloss(_))),
        "the gloss joined the existing lens"
    );
}

/// The rename flow through the omnibar: BeginRenameSession opens the bar in
/// rename mode seeded with the current label; commit lowers RenameSession
/// and the label updates. An empty name clears back to the derived/uuid
/// fallback.
#[test]
fn rename_session_through_the_omnibar_mode() {
    use crate::ui::OmnibarMode;

    let mut app = App::test_stub();
    let id = app.session_id;
    app.sessions
        .insert(session_runtime::GraphSessionManifest::new(
            id,
            GraphId::nil(),
        ));
    // The default label is the uuid prefix.
    assert_eq!(app.session_label(id), id.as_uuid().to_string()[..8]);

    app.update(Action::BeginRenameSession);
    assert!(app.omnibar.open);
    assert!(matches!(app.omnibar.mode, OmnibarMode::RenameSession(rid) if rid == id));

    // Type a new name over the seeded label and commit.
    app.omnibar.text = "Research".to_string();
    app.update(Action::OmnibarCommit);
    assert_eq!(app.session_label(id), "Research");
    assert!(!app.omnibar.open, "commit closes the bar");
    assert!(
        matches!(app.omnibar.mode, OmnibarMode::Address),
        "mode resets"
    );

    // An empty rename clears back to the uuid fallback.
    app.update(Action::RenameSession {
        id,
        name: "   ".to_string(),
    });
    assert_eq!(app.session_label(id), id.as_uuid().to_string()[..8]);
}

/// The recycle-bin round trip, app-level (the port simulated by folding
/// its answers): delete stages the focused node's record — ORIGINAL id,
/// title, tags — and drops the node; the Trail derives it into Removed;
/// recover re-mints under the SAME id and Removed derives it away.
#[test]
fn delete_stages_into_the_bin_and_recover_restores_identity() {
    use crate::trail_view::{RowAction, trail_rows};

    let mut app = App::test_stub();
    let url = "https://example.com/gone".to_string();
    app.update(Action::OpenAddress(url.clone()));
    let original = app
        .canvas
        .focused_member()
        .expect("the opened node is focused");

    let fx = app.update(Action::DeleteFocusedNode);
    assert!(
        app.canvas.graph().get_node_by_url(&url).is_none(),
        "the node left the graph"
    );
    // The record leaves through the bin port carrying the identity.
    let record = fx
        .iter()
        .find_map(|e| match e {
            Effect::RecordDeleted { record } => Some(record.clone()),
            _ => None,
        })
        .expect("delete stages a bin record: {fx:?}");
    assert_eq!(
        record.node_id, original,
        "the record carries the ORIGINAL id"
    );
    assert_eq!(record.url, url);
    assert!(
        fx.iter().any(|e| matches!(e, Effect::CloseContent { .. })),
        "its content session is closed: {fx:?}"
    );

    // The port answers with the refreshed list (folded as the drain would).
    app.apply_update(Update::BinListed {
        records: vec![record],
    });
    assert!(
        trail_rows(&app).iter().any(
            |r| matches!(&r.action, RowAction::Recover(id) if id == &original.to_string())
        ),
        "the staged node derives into the Trail's Removed section"
    );

    // Recover BY IDENTITY: same uuid, and Removed derives it away with the
    // record still in the bin (append-only until athanor's pass).
    app.update(Action::RecoverDeletedNode(original));
    assert_eq!(
        app.canvas.focused_member(),
        Some(original),
        "the node is back under its ORIGINAL id, selected"
    );
    assert!(
        app.canvas.graph().get_node_by_url(&url).is_some(),
        "the url resolves again"
    );
    assert!(
        !trail_rows(&app)
            .iter()
            .any(|r| matches!(&r.action, RowAction::Recover(_))),
        "Removed derives away once the node is present (record still staged)"
    );
    assert!(!app.removed.is_empty(), "the bin record itself remains");
}

/// The envelope lane end to end (participant gate B3): a dropped `.wasm`
/// installs as a component denizen after the same VISIBLE review — whose
/// row now names its ring profile — and running it lowers exactly the
/// emissions its grant covers, attributed, while the ungranted ring and
/// gate management are refused inside the run.
#[cfg(feature = "wasm")]
#[test]
fn a_component_denizen_acts_only_within_its_reviewed_rings() {
    let mut app = App::test_stub();
    app.data_root =
        std::env::temp_dir().join(format!("turnstone-component-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&app.data_root);
    std::fs::create_dir_all(app.session_dir()).unwrap();
    let pack = std::path::Path::new("scenarios/fixtures/app_core_guest.wasm");
    assert!(
        pack.exists(),
        "the app-core guest fixture is missing at {}",
        pack.display()
    );

    // Stage: the review names the component and its preselected rings.
    app.update(Action::InstallDenizen { path: pack.display().to_string() });
    let review = &app.denizen_actions()[0].0;
    assert!(review.contains("(wasm)"), "the lane is named: {review}");
    for ring in ["navigate", "panes", "dispatch"] {
        assert!(review.contains(ring), "the ask names {ring}: {review}");
    }
    assert!(
        !review.contains("session"),
        "the destructive ring is never preselected: {review}"
    );

    app.update(Action::ConfirmInstallDenizen);
    let (member, subject) = {
        let (&m, r) = app.denizens.residents.iter().next().unwrap();
        (m, r.subject)
    };
    let binding = session_runtime::read_denizen_binding(app.canvas.facets(), member).unwrap();
    assert_eq!(binding.kind, session_runtime::DenizenKind::Pack);
    let file = app
        .canvas
        .facets()
        .get(&member, &chartulary::FacetId::new(crate::denizen::COMPONENT_FACET))
        .and_then(|v| v.as_str().map(str::to_string))
        .expect("the component facet points at the stored bytes");
    assert!(
        crate::denizen::component_path(&app.session_dir(), &file).is_file(),
        "the component's bytes live beside the worlds"
    );
    // The grant is exactly the reviewed rings: each ring is its own power,
    // and there is no capability above them that could grant them wholesale.
    let covers = |ring: crate::ring::Ring| {
        servitor::AuthorityProvider::covers(
            &app.denizens.authority,
            subject,
            &ring.cap().expect("a grantable ring"),
            servitor::Mode::Write,
        )
    };
    use crate::ring::Ring;
    assert!(covers(Ring::Navigate) && covers(Ring::Panes) && covers(Ring::Dispatch));
    assert!(!covers(Ring::Session), "an unreviewed ring is ungranted");

    // Run: the guest emits one action per ring. Only the covered ones land.
    let before = app.canvas.graph().node_count();
    app.update(Action::RunDenizen { member });
    assert!(
        app.canvas.graph().get_node_by_url("mere://kept/note").is_some(),
        "the navigate emission lowered through the spine"
    );
    assert_eq!(
        app.canvas.graph().node_count(),
        before + 1,
        "and nothing else minted a node"
    );
    assert!(
        app.take_events()
            .iter()
            .any(|e| matches!(e, AppEvent::DenizenRan(_))),
        "the run is observable"
    );
    // Attribution: the component's edit reads back under its subject.
    let journal = app.journal.lock().unwrap();
    assert!(
        journal
            .entries()
            .iter()
            .any(|entry| entry.author == subject.to_hex()),
        "the component's graph edit is attributed to its subject"
    );
    drop(journal);
    let _ = std::fs::remove_dir_all(&app.data_root);
}

/// The re-root heal: when the profile's root identity changes (the vault
/// superseding the unsealed stopgap key), stored certificates fail as
/// WrongRoot — and the adopt path re-issues from the grant projections
/// under the NEW root, preserving exactly the reviewed grant. No denizen
/// silently loses authority to a key migration.
#[test]
fn a_rerooted_profile_reissues_delegations_from_the_reviewed_projections() {
    use servitor::AuthorityProvider;

    let mut app = App::test_stub();
    app.data_root =
        std::env::temp_dir().join(format!("turnstone-reroot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&app.data_root);
    std::fs::create_dir_all(app.session_dir()).unwrap();
    let pack = app.data_root.join("keeper.lua");
    std::fs::write(&pack, "mere.open('mere://kept/note')").unwrap();
    app.update(Action::InstallDenizen { path: pack.display().to_string() });
    app.update(Action::ConfirmInstallDenizen);
    let subject = app.denizens.residents.values().next().unwrap().subject;
    let navigate = crate::ring::Ring::Navigate.cap().unwrap();
    let session_ring = crate::ring::Ring::Session.cap().unwrap();

    // The profile re-roots: a NEW identity (the vault superseding the
    // stopgap seed). The old certificates on disk name the old root.
    let new_root = identity::InMemoryProvider::from_seed([77u8; 32]);
    let rebuilt = crate::denizen::rebuild(
        app.canvas.facets(),
        app.canvas.graph(),
        &app.session_dir(),
        &new_root,
    );
    assert_eq!(rebuilt.residents.len(), 1, "the resident survives the migration");
    assert!(
        rebuilt.authority.covers(subject, &navigate, servitor::Mode::Write),
        "the reviewed ring re-rooted under the new identity"
    );
    assert!(
        !rebuilt.authority.covers(subject, &session_ring, servitor::Mode::Write),
        "and the heal preserves the review exactly: nothing widens"
    );
    // Durable: the certificate file was rewritten under the new root, so
    // the NEXT adopt verifies without healing.
    let stored = crate::denizen::load_certs(&app.session_dir(), &subject.to_hex());
    assert!(!stored.is_empty());
    assert!(
        stored.iter().all(|signed| {
            signed.certificate.issuer
                == identity::IdentityProvider::master_public_key(&new_root).to_bytes()
        }),
        "the stored chain now roots at the new identity"
    );
    let _ = std::fs::remove_dir_all(&app.data_root);
}

/// Install is a signed delegation from the profile identity, and uninstall
/// REVOKES it (capability-model C4). The arc: install grants exactly the
/// reviewed rings, uninstall revokes them and un-resides the denizen, and
/// what it was authorized to do stops being authorized — without
/// destroying its node or its world.
#[test]
fn install_delegates_from_the_profile_identity_and_uninstall_revokes_it() {
    use servitor::AuthorityProvider;

    let mut app = App::test_stub();
    app.data_root =
        std::env::temp_dir().join(format!("turnstone-revoke-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&app.data_root);
    std::fs::create_dir_all(app.session_dir()).unwrap();
    // The root identity is the profile's, not a constant.
    let root = identity::IdentityProvider::master_public_key(app.identity.as_ref()).to_bytes();
    assert_eq!(app.denizens.authority.root(), root, "rooted on the profile identity");

    let pack = app.data_root.join("keeper.lua");
    std::fs::write(&pack, "mere.open('mere://kept/note')").unwrap();
    app.update(Action::InstallDenizen { path: pack.display().to_string() });
    app.update(Action::ConfirmInstallDenizen);
    let (member, subject) = {
        let (&m, r) = app.denizens.residents.iter().next().unwrap();
        (m, r.subject)
    };
    let navigate = crate::ring::Ring::Navigate.cap().unwrap();
    let session_ring = crate::ring::Ring::Session.cap().unwrap();
    assert!(
        app.denizens.authority.covers(subject, &navigate, servitor::Mode::Write),
        "the reviewed ring is authorized by a verified certificate chain"
    );
    assert!(
        !app.denizens.authority.covers(subject, &session_ring, servitor::Mode::Write),
        "an unreviewed ring was never delegated"
    );
    let certs = crate::denizen::certs_path(&app.session_dir(), &subject.to_hex());
    assert!(certs.is_file(), "the signed delegations persisted");

    // Uninstall: revoke and un-reside.
    app.update(Action::UninstallDenizen { member });
    assert!(app.denizens.residents.is_empty(), "no longer resident");
    assert!(
        !app.denizens.authority.covers(subject, &navigate, servitor::Mode::Write),
        "the delegation is revoked, so the ring is no longer authorized"
    );
    assert!(
        session_runtime::read_denizen_binding(app.canvas.facets(), member).is_none(),
        "un-resided: the agency facet is gone"
    );
    assert!(!certs.is_file(), "and its certificates cannot resurrect it on adopt");
    assert!(
        app.take_events()
            .iter()
            .any(|e| matches!(e, AppEvent::DenizenUninstalled(_))),
        "the uninstall is observable"
    );
    // Nothing was destroyed: the node and its borne world remain.
    assert!(
        app.canvas.graph().get_node_by_id(member).is_some(),
        "revoking authority does not delete the node"
    );
    assert!(
        crate::denizen::nested_log_path(&app.session_dir(), &subject.to_hex()).is_file(),
        "nor its world"
    );
    // And the palette no longer offers to run it.
    assert!(
        !app.denizen_actions().iter().any(|(label, _)| label.starts_with("Run ")),
        "the Run row is gone with the residency"
    );
    let _ = std::fs::remove_dir_all(&app.data_root);
}

/// Archive-never-orphan at the node tier: deleting a denizen node moves
/// its world file to the archive slot (nothing orphaned in the live dir,
/// nothing destroyed), the tombstone carries the world id + facet bundle,
/// and recovery restores full residency — world back live, binding facet
/// back, resident rebuilt.
#[test]
fn deleting_a_denizen_archives_its_world_and_recovery_restores_residency() {
    let mut app = App::test_stub();
    app.data_root =
        std::env::temp_dir().join(format!("turnstone-bin-world-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&app.data_root);
    std::fs::create_dir_all(app.session_dir()).unwrap();
    let pack = app.data_root.join("keeper.lua");
    std::fs::write(&pack, "mere.open('mere://kept/note')").unwrap();
    app.update(Action::InstallDenizen { path: pack.display().to_string() });
    app.update(Action::ConfirmInstallDenizen);
    let (member, world_id, world_revision) = {
        let (&member, resident) = app.denizens.residents.iter().next().unwrap();
        (member, resident.subject.to_hex(), resident.nested.revision())
    };
    assert!(
        crate::denizen::nested_log_path(&app.session_dir(), &world_id).is_file(),
        "the world is live before the delete"
    );

    // Delete: the install left the denizen node selected.
    let fx = app.update(Action::DeleteFocusedNode);
    let record = fx
        .iter()
        .find_map(|e| match e {
            Effect::RecordDeleted { record } => Some(record.clone()),
            _ => None,
        })
        .expect("delete stages a bin record: {fx:?}");
    assert_eq!(record.node_id, member);
    assert_eq!(record.nested.as_deref(), Some(world_id.as_str()));
    assert!(
        record
            .facets
            .as_ref()
            .and_then(|f| f.get(session_runtime::DENIZEN_BINDING))
            .is_some(),
        "the tombstone carries the facet bundle incl. the binding"
    );
    assert!(
        !crate::denizen::nested_log_path(&app.session_dir(), &world_id).is_file(),
        "the live slot is empty"
    );
    assert!(
        crate::denizen::archived_world_path(&app.session_dir(), &world_id).is_file(),
        "the world moved to the archive slot, never orphaned"
    );
    assert!(
        app.denizens.residents.is_empty(),
        "the runtime entry left with the node"
    );
    assert!(
        session_runtime::read_denizen_binding(app.canvas.facets(), member).is_none(),
        "the live facets went to the tombstone"
    );

    // Recover: full residency returns.
    app.apply_update(Update::BinListed { records: vec![record] });
    app.update(Action::RecoverDeletedNode(member));
    assert!(
        crate::denizen::nested_log_path(&app.session_dir(), &world_id).is_file(),
        "the world is live again"
    );
    assert!(
        !crate::denizen::archived_world_path(&app.session_dir(), &world_id).is_file(),
        "the archive slot emptied"
    );
    assert!(
        session_runtime::read_denizen_binding(app.canvas.facets(), member).is_some(),
        "the binding facet restored"
    );
    let resident = app
        .denizens
        .residents
        .get(&member)
        .expect("the recovered denizen resides again");
    assert_eq!(
        resident.nested.revision(),
        world_revision,
        "the same world, not a fresh one"
    );
    let _ = std::fs::remove_dir_all(&app.data_root);
}

/// Empty-the-bin is athanor's oven on command: it lowers the EmptyRecycleBin
/// effect (the actor clears the store) only when there is something to
/// forget, and folding the port's empty answer clears the mirror.
#[test]
fn empty_recycle_bin_forgets_on_command() {
    let mut app = App::test_stub();
    // An empty bin is a no-op: no effect, no event (honest, no placebo).
    let fx = app.update(Action::EmptyRecycleBin);
    assert!(
        !fx.iter().any(|e| matches!(e, Effect::EmptyRecycleBin)),
        "nothing to empty lowers no effect: {fx:?}"
    );

    // Stage two records (as the bin port's answer would), then empty.
    app.apply_update(Update::BinListed {
        records: vec![
            crate::action::RemovedRecord {
                node_id: uuid::Uuid::new_v4(),
                url: "https://a.test".into(),
                title: None,
                tags: Vec::new(),
                deleted_at_ms: 2,
                nested: None,
                facets: None,
            },
            crate::action::RemovedRecord {
                node_id: uuid::Uuid::new_v4(),
                url: "https://b.test".into(),
                title: None,
                tags: Vec::new(),
                deleted_at_ms: 1,
                nested: None,
                facets: None,
            },
        ],
    });
    let fx = app.update(Action::EmptyRecycleBin);
    assert!(
        fx.iter().any(|e| matches!(e, Effect::EmptyRecycleBin)),
        "a non-empty bin lowers the clear effect: {fx:?}"
    );
    // The store's empty answer (folded as the drain would) clears the mirror.
    app.apply_update(Update::BinListed {
        records: Vec::new(),
    });
    assert!(
        app.removed.is_empty(),
        "the mirror is empty after the bin clears"
    );
}

/// The rung7_lens_ops receipt's exact op sequence, app-level: tear out
/// the roster, summon the trail beside it (in the lens), reweight, close
/// the ACTIVE pane. The close must remove the TRAIL (the summon made it
/// active), never the roster.
#[test]
/// The gloss-composite's add/remove: a Gloss pane starts as a bare
/// minimap, the palette offers pane-scoped section rows, toggling one
/// edits THAT LEAF (so it persists with the layout), and toggling again
/// removes it.
#[test]
fn composing_a_gloss_pane_toggles_sections_on_its_own_leaf() {
    let mut app = App::test_stub();
    app.update(Action::SummonPane(PaneKind::Gloss));
    let pane = app.active_pane.expect("the summoned gloss is active");

    // At base it is a minimap: no composed sections.
    let sections = |app: &App| match app.pane_content(pane) {
        Some(PaneContent::Gloss(cfg)) => cfg.sections.clone(),
        _ => panic!("the active pane is a Gloss"),
    };
    assert!(sections(&app).is_empty(), "base is a bare minimap");

    // The palette offers an ADD row per provider while it is active.
    let offered = app.session_actions();
    assert!(
        offered
            .iter()
            .any(|(label, _)| label == "Gloss: add section — Removed"),
        "pane-scoped add row is offered: {offered:?}"
    );

    // Toggling composes it onto the leaf, and persists (SaveSession).
    let fx = app.update(Action::TogglePaneSection {
        pane,
        section: "removed".to_string(),
    });
    assert_eq!(sections(&app), vec!["removed".to_string()]);
    assert!(
        fx.iter().any(|e| matches!(e, Effect::SaveSession)),
        "the composition persists with the layout: {fx:?}"
    );
    // Now the palette offers REMOVE for it.
    assert!(
        app.session_actions()
            .iter()
            .any(|(label, _)| label == "Gloss: remove section — Removed")
    );

    // Toggling again removes it, back to the bare minimap.
    app.update(Action::TogglePaneSection {
        pane,
        section: "removed".to_string(),
    });
    assert!(sections(&app).is_empty(), "toggled back off");
}

/// One catalog, offered in one order: the contextual rows LEAD the static
/// registry (a pending grant review must be first), and every consumer
/// reads this same list — the `>` lane filters it, the snapshot reports it,
/// and the automation runner resolves a label through it. Composing it
/// twice is how the runner and the palette come to disagree.
#[test]
fn available_actions_lead_with_the_contextual_rows() {
    let mut app = App::test_stub();
    app.update(Action::SummonPane(PaneKind::Gloss));
    let rows = app.available_actions();

    // The contextual rows (here, the active Gloss's composition rows) come
    // first; the static registry follows.
    let first_static = rows
        .iter()
        .position(|(label, _)| label == "Fit view")
        .expect("the static registry is in the catalog");
    let a_contextual = rows
        .iter()
        .position(|(label, _)| label.starts_with("Gloss: add section"))
        .expect("the active pane's rows are in the catalog");
    assert!(
        a_contextual < first_static,
        "contextual rows lead the static registry: {rows:?}"
    );

    // The catalog is exactly the two sources, nothing invented or dropped.
    assert_eq!(
        rows.len(),
        app.session_actions().len() + crate::action::palette_actions().len()
    );

    // And the snapshot reports THAT list, by label and in that order, so an
    // automation lane sees what a person would.
    let snap = crate::observe::snapshot(&app);
    let labels: Vec<String> = rows.into_iter().map(|(label, _)| label).collect();
    assert_eq!(snap.available_actions, labels);
}

#[test]
fn publishing_pane_is_a_summonable_window_control() {
    let mut app = App::test_stub();
    app.update(Action::SummonPane(PaneKind::Publishing));

    assert!(app.frisket.iter_leaves().any(|(_, content, _)| {
        matches!(content, PaneContent::Custom(name) if name == "publishing")
    }));
    assert!(crate::action::palette_actions().iter().any(|(label, action)| {
        *label == "Open Publishing pane"
            && matches!(action, Action::SummonPane(PaneKind::Publishing))
    }));
}

/// Composition ORDER is the config's order, so reordering is the same leaf
/// edit as add/remove: it moves within the stack, clamps at the ends
/// rather than wrapping, and the palette only offers a move that would do
/// something.
#[test]
fn moving_a_composed_section_reorders_that_leaf_and_clamps() {
    let mut app = App::test_stub();
    app.update(Action::SummonPane(PaneKind::Gloss));
    let pane = app.active_pane.expect("the summoned gloss is active");
    let sections = |app: &App| match app.pane_content(pane) {
        Some(PaneContent::Gloss(cfg)) => cfg.sections.clone(),
        _ => panic!("the active pane is a Gloss"),
    };
    let mv = |app: &mut App, section: &str, delta: i32| {
        app.update(Action::MovePaneSection {
            pane,
            section: section.to_string(),
            delta,
        })
    };

    // With ONE section there is nothing to reorder, so no move row.
    app.update(Action::TogglePaneSection {
        pane,
        section: "removed".to_string(),
    });
    assert!(
        !app.session_actions()
            .iter()
            .any(|(label, _)| label.starts_with("Gloss: move section")),
        "a lone section offers no move"
    );

    // Compose a second: it stacks BELOW, in config order.
    app.update(Action::TogglePaneSection {
        pane,
        section: "nodes".to_string(),
    });
    assert_eq!(sections(&app), vec!["removed", "nodes"]);

    // Moving it up swaps the stack, and persists with the layout.
    let fx = mv(&mut app, "nodes", -1);
    assert_eq!(sections(&app), vec!["nodes", "removed"]);
    assert!(
        fx.iter().any(|e| matches!(e, Effect::SaveSession)),
        "a reorder persists like any leaf edit: {fx:?}"
    );

    // At the top, up is a no-op: clamped, NOT wrapped to the bottom. It
    // reports no move, so the receipt cannot mistake it for one.
    let fx = mv(&mut app, "nodes", -1);
    assert_eq!(sections(&app), vec!["nodes", "removed"], "clamped at the top");
    assert!(
        !fx.iter().any(|e| matches!(e, Effect::SaveSession)),
        "a no-op move saves nothing: {fx:?}"
    );
    // And the palette does not offer it.
    assert!(
        !app.session_actions()
            .iter()
            .any(|(label, _)| label == "Gloss: move section up — Nodes"),
        "no up-row on the first section"
    );

    // An id this pane has not composed moves nothing.
    let fx = mv(&mut app, "recent", 1);
    assert_eq!(sections(&app), vec!["nodes", "removed"]);
    assert!(!fx.iter().any(|e| matches!(e, Effect::SaveSession)));
}

/// Composition is a property of a PANE, not of the Gloss: the Overmap
/// composes the same sections, through the same renderer, with the same
/// leaf config and the same actions. Its palette rows name it, derived from
/// the pane's own tag rather than a second table.
#[test]
fn the_overmap_composes_sections_too() {
    let mut app = App::test_stub();
    app.update(Action::SummonPane(PaneKind::Overmap));
    let pane = app.active_pane.expect("the summoned overmap is active");

    // A fresh Overmap composes nothing (its swatch fills the pane).
    assert_eq!(
        app.pane_content(pane).and_then(|c| c.composition()),
        Some(&crate::panes::PaneComposition::default())
    );
    // The palette offers ITS rows, named for IT.
    assert!(
        app.session_actions()
            .iter()
            .any(|(label, _)| label == "Overmap: add section — Removed"),
        "the pane-scoped rows name the pane: {:?}",
        app.session_actions()
    );

    // The same action composes it, onto ITS OWN leaf.
    app.update(Action::TogglePaneSection {
        pane,
        section: "removed".to_string(),
    });
    match app.pane_content(pane) {
        Some(PaneContent::Overmap(cfg)) => assert_eq!(cfg.sections, vec!["removed"]),
        other => panic!("expected a composed overmap, got {other:?}"),
    }
}

fn lens_ops_close_removes_the_summoned_pane() {
    let mut app = App::test_stub();
    app.update(Action::SummonPane(PaneKind::Roster));
    app.update(Action::TearOutActivePane);
    app.update(Action::SummonPane(PaneKind::Trail));
    app.update(Action::SetActivePaneDivider(0.7));
    app.update(Action::CloseActivePane);
    let lens = app.lenses[0].as_ref().unwrap();
    let tags: Vec<&str> = lens.iter_leaves().map(|(_, c, _)| c.tag()).collect();
    assert!(
        tags.contains(&"roster") && !tags.contains(&"trail"),
        "close removes the summoned trail, not the roster: {tags:?}"
    );
}

/// The nav row (r3 owed): Back re-selects without refetching, Forward
/// redoes, a new open truncates the forward branch, and Reload refetches
/// the focused node and respawns its live content.
#[test]
fn back_forward_and_reload_flow_through_the_spine() {
    let mut app = App::test_stub();
    app.update(Action::OpenAddress("https://example.com/a".to_string()));
    app.update(Action::OpenAddress("https://example.com/b".to_string()));
    // Back: the previous node re-selects, with NO fetch effect.
    let effects = app.update(Action::NavBack);
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::FetchPage { .. })),
        "Back never refetches: {effects:?}"
    );
    assert_eq!(app.canvas.focused_url(), Some("https://example.com/a"));
    // Forward redoes.
    app.update(Action::NavForward);
    assert_eq!(app.canvas.focused_url(), Some("https://example.com/b"));
    // Back then a new open: the forward branch truncates.
    app.update(Action::NavBack);
    app.update(Action::OpenAddress("https://example.com/c".to_string()));
    assert!(!app.history.can_forward(), "a new open truncates forward");
    assert!(app.history.can_back());
    // Reload: a fetch effect for the focused node; with live content, a
    // close + respawn pair.
    let node = app.canvas.focused_member().unwrap();
    app.apply_update(Update::ContentSpawned { node, facts: None });
    let effects = app.update(Action::Reload);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::FetchPage { .. }))
    );
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::CloseContent { .. }))
    );
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::SpawnContent { .. }))
    );
    let described: Vec<String> = app
        .take_events()
        .iter()
        .map(crate::observe::AppEvent::describe)
        .collect();
    assert!(described.iter().any(|e| e.starts_with("nav-back ")));
    assert!(described.iter().any(|e| e.starts_with("nav-forward ")));
    assert!(described.iter().any(|e| e.starts_with("reloaded ")));
}

/// The workbench lane end to end at the App tier: opening the focused
/// node tiles it, summons the Workbench pane, and spawns its content;
/// stacking collapses cells; closing empties honestly.
#[test]
fn workbench_actions_flow_through_the_spine() {
    let mut app = App::test_stub();
    app.update(Action::OpenAddress("mere://alpha".to_string()));
    let a = app.canvas.focused_member().unwrap();
    let effects = app.update(Action::OpenInWorkbench);
    assert!(app.workbench.is_tiled());
    assert_eq!(app.workbench.tile_count(), 1);
    assert!(
        app.frisket
            .iter_leaves()
            .any(|(_, c, _)| matches!(c, PaneContent::Workbench)),
        "opening a tile summons the Workbench pane"
    );
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::SpawnContent { .. })),
        "a tile wants live content: {effects:?}"
    );
    // Re-opening the same node adds nothing.
    app.update(Action::OpenInWorkbench);
    assert_eq!(app.workbench.tile_count(), 1);
    // A second node tiles beside it; stacking collapses to one cell.
    app.update(Action::OpenAddress("mere://beta".to_string()));
    let b = app.canvas.focused_member().unwrap();
    app.update(Action::OpenInWorkbench);
    assert_eq!(app.workbench.slot_count(), 2);
    app.update(Action::WorkbenchStackOnto {
        dragged: b,
        target: a,
    });
    assert_eq!(app.workbench.slot_count(), 1);
    assert_eq!(app.workbench.tile_count(), 2);
    // Activate the buried tab; close the focused (beta) tile.
    app.update(Action::WorkbenchActivate(a));
    app.update(Action::CloseWorkbenchTile);
    assert_eq!(app.workbench.tile_count(), 1);
    assert!(app.workbench.has_tile(a));
}

/// The browser-state sidecar (rung 6): content-on mirrors live truth at
/// refresh, prunes vanished nodes, and round-trips through the store.
#[test]
fn browser_states_refresh_and_round_trip() {
    let mut app = App::test_stub();
    app.update(Action::OpenAddress("https://example.com/a".to_string()));
    let a = app.canvas.focused_member().unwrap();
    app.apply_update(Update::ContentSpawned {
        node: a,
        facts: None,
    });
    app.update(Action::OpenAddress("https://example.com/b".to_string()));
    app.refresh_browser_states();
    assert!(app.browser.get(a).is_some_and(|b| b.content_on));
    assert!(
        app.browser
            .get(app.canvas.focused_member().unwrap())
            .is_none(),
        "a node without content stays out of the sidecar"
    );
    // Round trip through the converged store: web.* facets in facets.json.
    let dir = std::env::temp_dir().join(format!("turnstone-bn-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut facets = session_runtime::NodeFacetStore::new();
    session_runtime::write_web_states(&mut facets, &app.browser);
    crate::session::save_node_facets(&dir, &facets);
    let reloaded = crate::session::load_node_facets(&dir).unwrap_or_default();
    let restored = session_runtime::read_web_states(&reloaded);
    assert!(restored.get(a).is_some_and(|b| b.content_on));
    // Content off -> the refresh clears the flag.
    app.content.note_closed(a);
    app.refresh_browser_states();
    assert!(
        !app.browser.get(a).is_some_and(|b| b.content_on),
        "closed content clears the flag on the next refresh"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The workbench sidecar round-trips through the persistence port,
/// pruned to present members (platen's canonical pair underneath).
#[test]
fn workbench_persists_and_restores_pruned() {
    let dir = std::env::temp_dir().join(format!("turnstone-wb-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
    let mut wb = mere::platen::Workbench::new();
    wb.ensure_tiled();
    wb.open_tile(a);
    wb.open_tile(b);
    crate::session::save_workbench(&dir, &wb);
    // Both present: both tiles come back.
    let present: std::collections::HashSet<_> = [a, b].into_iter().collect();
    let restored = crate::session::load_workbench(&dir, &present);
    assert_eq!(restored.tile_count(), 2);
    // b's node vanished between sessions: its tile is reconciled away.
    let present: std::collections::HashSet<_> = [a].into_iter().collect();
    let restored = crate::session::load_workbench(&dir, &present);
    assert_eq!(restored.tile_count(), 1);
    assert!(restored.has_tile(a));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Committing a find-lane node row selects without fetching.
#[test]
fn committing_a_node_row_selects_without_fetch_effects() {
    let mut app = App::test_stub();
    app.canvas.visit("https://example.com/meerkats");
    app.update(Action::OmnibarOpen { command: false });
    for c in "meer".chars() {
        app.update(Action::OmnibarChar(c));
    }
    let effects = app.update(Action::OmnibarCommit);
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::FetchPage { .. })),
        "selecting an existing node must not refetch: {effects:?}"
    );
    assert!(!app.omnibar.open);
}
