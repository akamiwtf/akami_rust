use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use serde_json::{json, Value};

use crate::config::Config;
use crate::crypto::DmCrypto;
use crate::db::Db;
use crate::voice::VoiceRegistry;

/// Emits realtime events. Installed by the Socket.io layer at startup; while
/// unset (e.g. in REST-only tests) every emit is a no-op.
pub type Broadcaster = Arc<dyn Fn(EmitTarget<'_>, &str, Value) + Send + Sync>;

pub enum EmitTarget<'a> {
    All,
    Room(&'a str),
}

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub config: Config,
    pub dm_crypto: DmCrypto,
    pub voice: Arc<VoiceRegistry>,
    broadcaster: Arc<RwLock<Option<Broadcaster>>>,
    /// Ephemeral Rich Presence per user id (game the user is playing).
    /// Not persisted — lives only while the RPC client is connected.
    presence: Arc<RwLock<HashMap<String, Value>>>,
}

/// The rich-presence fields a client can report. `app_name` empty ⇒ cleared.
pub const RPC_FIELDS: [&str; 5] = [
    "rpcAppName",
    "rpcDetails",
    "rpcState",
    "rpcStartTime",
    "rpcLargeImage",
];

impl AppState {
    pub fn new(db: Db, config: Config) -> Self {
        let dm_crypto = DmCrypto::new(&config.dm_encryption_key);
        Self {
            db,
            config,
            dm_crypto,
            voice: Arc::new(VoiceRegistry::default()),
            broadcaster: Arc::new(RwLock::new(None)),
            presence: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Stores (or replaces) a user's presence. `presence` should carry the
    /// `rpc*` fields; `userId` is added automatically.
    pub fn set_presence(&self, user_id: &str, mut presence: Value) {
        if let Value::Object(map) = &mut presence {
            map.insert("userId".into(), json!(user_id));
        }
        self.presence
            .write()
            .unwrap()
            .insert(user_id.to_string(), presence);
    }

    /// Drops a user's presence. Returns whether anything was removed.
    pub fn clear_presence(&self, user_id: &str) -> bool {
        self.presence.write().unwrap().remove(user_id).is_some()
    }

    /// A cleared-presence payload (all rpc fields null) for the given user.
    pub fn cleared_presence(user_id: &str) -> Value {
        let mut obj = json!({ "userId": user_id });
        for f in RPC_FIELDS {
            obj[f] = Value::Null;
        }
        obj
    }

    /// Snapshot of every active presence, for syncing a freshly-connected socket.
    pub fn presence_snapshot(&self) -> Vec<Value> {
        self.presence.read().unwrap().values().cloned().collect()
    }

    /// Copies the stored `rpc*` fields onto a serialized user object (in place).
    /// No-op when the user has no active presence.
    pub fn apply_presence(&self, user: &mut Value) {
        let Some(id) = user.get("id").and_then(Value::as_str).map(String::from) else {
            return;
        };
        if let Some(p) = self.presence.read().unwrap().get(&id) {
            for f in RPC_FIELDS {
                if let Some(val) = p.get(f) {
                    user[f] = val.clone();
                }
            }
        }
    }

    /// Installs the realtime broadcaster (called once the Socket.io server exists).
    pub fn set_broadcaster(&self, b: Broadcaster) {
        *self.broadcaster.write().unwrap() = Some(b);
    }

    pub fn emit_all(&self, event: &str, payload: Value) {
        if let Some(b) = self.broadcaster.read().unwrap().as_ref() {
            b(EmitTarget::All, event, payload);
        }
    }

    pub fn emit_to(&self, room: &str, event: &str, payload: Value) {
        if let Some(b) = self.broadcaster.read().unwrap().as_ref() {
            b(EmitTarget::Room(room), event, payload);
        }
    }
}
