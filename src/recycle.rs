//! The recycle-bin port: the eidetic deleted-node bin behind an armillary
//! actor (design_docs/2026-07-20_recycle_bin_athanor.md, slice 1).
//!
//! The bin IS `eidetic::deleted` — `DeletedNode` records staged into a
//! session-scoped `eidetic_fjall::FjallStore` at `sessions/<id>/bin`. This
//! module is the port adapter on merecat's spine: the app lowers
//! [`Effect::RecordDeleted`](crate::action::Effect), the shell forwards a
//! [`BinCommand`] to the actor, and the actor answers with app-owned
//! [`Update`]s ([`Update::BinListed`] / [`Update::BinFailed`]) — eidetic's
//! concrete types never cross the boundary (the port-agnostic rule).
//!
//! The actor answers EVERY command — record, reopen, and its own spawn —
//! with the refreshed list, so the app's cache can never sit stale behind a
//! write, and a store failure is a loud [`Update::BinFailed`], never an empty
//! list masquerading as "nothing deleted". Store ops run under
//! [`pollster::block_on`] on the actor thread: they are serial disk IO over
//! one LSM store, which wants ordering, not a runtime.
//!
//! Athanor (the oven: permanent forgetting + engram bake, on command or
//! schedule) is slice 3 and will speak to the same actor.

use std::path::{Path, PathBuf};

use armillary::{ActorHandle, Emitter, Wake, spawn_named};
use eidetic::{DeletedNode, Store, list_deleted, record_deleted};
use eidetic_fjall::FjallStore;
use std::sync::mpsc::Receiver;

use crate::action::{RemovedRecord, Update};

/// Commands the bin actor takes (the shell sends these; ordering on the one
/// channel is the consistency story).
pub enum BinCommand {
    /// Stage a removed node, then answer with the refreshed list.
    Record(RemovedRecord),
    /// Re-point the store at another session's bin (a session switch), then
    /// answer with ITS list.
    Reopen(PathBuf),
    /// Drop the open store and ack — the close path's handshake: Windows
    /// cannot rename a directory whose files are open, so the shell releases
    /// the bin BEFORE moving the session dir to the trash. No list is emitted
    /// (the store is closed); the follow-up Reopen answers with the adopted
    /// session's list.
    Release(std::sync::mpsc::SyncSender<()>),
}

/// One session's bin directory (under its `sessions/<id>/` dir).
pub fn bin_dir(session_dir: &Path) -> PathBuf {
    session_dir.join("bin")
}

/// eidetic's record, in app-owned terms. A record whose `node_id` fails to
/// parse is dropped with a warn (a foreign or corrupt record must not wedge
/// the whole list).
fn to_record(d: DeletedNode) -> Option<RemovedRecord> {
    let Ok(node_id) = d.node_id.parse::<uuid::Uuid>() else {
        tracing::warn!(node_id = %d.node_id, "recycle bin: unparseable node id; record skipped");
        return None;
    };
    Some(RemovedRecord {
        node_id,
        url: d.url,
        title: d.title,
        tags: d.tags,
        deleted_at_ms: d.deleted_at_ms,
    })
}

fn to_deleted(r: &RemovedRecord, graph_id: Option<String>) -> DeletedNode {
    DeletedNode {
        node_id: r.node_id.to_string(),
        url: r.url.clone(),
        title: r.title.clone(),
        tags: r.tags.clone(),
        graph_id,
        deleted_at_ms: r.deleted_at_ms,
    }
}

fn open(dir: &Path) -> Result<FjallStore, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    FjallStore::open(dir).map_err(|e| e.to_string())
}

/// List the bin and emit it (newest first), or emit the failure.
fn emit_list(store: &mut dyn Store, out: &Emitter<Update>) {
    match pollster::block_on(list_deleted(store)) {
        Ok(deleted) => {
            let mut records: Vec<RemovedRecord> =
                deleted.into_iter().filter_map(to_record).collect();
            records.sort_by_key(|r| std::cmp::Reverse(r.deleted_at_ms));
            out.emit(Update::BinListed { records });
        }
        Err(err) => out.emit(Update::BinFailed {
            error: format!("list: {err}"),
        }),
    }
}

/// Spawn the bin actor over the session bin at `dir`, waking the event loop
/// on every answer (the fetch actor's exact shape). Returns the command
/// handle plus the update receiver the shell drains.
pub fn spawn_bin(wake: Wake, dir: PathBuf) -> (ActorHandle<BinCommand>, Receiver<Update>) {
    spawn_named("recycle-bin", wake, move |commands, out: Emitter<Update>| {
        let mut store = match open(&dir) {
            Ok(mut store) => {
                emit_list(&mut store, &out);
                Some(store)
            }
            Err(err) => {
                out.emit(Update::BinFailed {
                    error: format!("open {}: {err}", dir.display()),
                });
                None
            }
        };
        while let Ok(command) = commands.recv() {
            match command {
                BinCommand::Record(record) => {
                    let Some(store) = store.as_mut() else {
                        out.emit(Update::BinFailed {
                            error: "record: the bin store is not open".to_string(),
                        });
                        continue;
                    };
                    // graph_id: sessions are directory-scoped (one graph per
                    // session dir), so the record needs no graph scoping here.
                    let deleted = to_deleted(&record, None);
                    if let Err(err) = pollster::block_on(record_deleted(store, &deleted)) {
                        out.emit(Update::BinFailed {
                            error: format!("record: {err}"),
                        });
                        continue;
                    }
                    emit_list(store, &out);
                }
                BinCommand::Release(ack) => {
                    store = None;
                    let _ = ack.send(());
                }
                BinCommand::Reopen(dir) => match open(&dir) {
                    Ok(mut fresh) => {
                        emit_list(&mut fresh, &out);
                        store = Some(fresh);
                    }
                    Err(err) => {
                        store = None;
                        out.emit(Update::BinFailed {
                            error: format!("reopen {}: {err}", dir.display()),
                        });
                    }
                },
            }
        }
    })
}
