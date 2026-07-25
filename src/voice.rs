//! In-memory voice channel state, mirroring the `voiceStates` /
//! `voiceMediaStates` maps in the original index.js. Entries are stored as
//! JSON objects so profile fields can be merged the same loose way Node did.

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::{json, Value};

#[derive(Default)]
pub struct VoiceRegistry {
    /// channelId -> array of voice-user objects.
    states: Mutex<HashMap<String, Vec<Value>>>,
    /// channelId -> now-playing media object.
    media: Mutex<HashMap<String, Value>>,
}

/// Profile keys copied into a voice entry on `profile_updated` / status change.
const PROFILE_KEYS: &[&str] = &["username", "avatar", "status", "bio"];

fn socket_id(entry: &Value) -> Option<&str> {
    entry.get("socketId").and_then(Value::as_str)
}

impl VoiceRegistry {
    pub fn add_user(&self, channel_id: &str, user: Value) {
        let mut states = self.states.lock().unwrap();
        states.entry(channel_id.to_string()).or_default().push(user);
    }

    /// Removes every entry for `sid` and drops emptied channels.
    /// Returns true if anything changed.
    pub fn remove_socket(&self, sid: &str) -> bool {
        let mut states = self.states.lock().unwrap();
        let mut changed = false;
        let channels: Vec<String> = states.keys().cloned().collect();
        for ch in channels {
            if let Some(list) = states.get_mut(&ch) {
                let before = list.len();
                list.retain(|u| socket_id(u) != Some(sid));
                if list.len() != before {
                    changed = true;
                    if list.is_empty() {
                        states.remove(&ch);
                    }
                }
            }
        }
        changed
    }

    /// Channels that currently contain this socket (used on disconnect).
    pub fn channels_with_socket(&self, sid: &str) -> Vec<String> {
        let states = self.states.lock().unwrap();
        states
            .iter()
            .filter(|(_, list)| list.iter().any(|u| socket_id(u) == Some(sid)))
            .map(|(ch, _)| ch.clone())
            .collect()
    }

    /// socketIds of other users in the channel (for P2P init).
    pub fn other_socket_ids(&self, channel_id: &str, sid: &str) -> Vec<String> {
        let states = self.states.lock().unwrap();
        states
            .get(channel_id)
            .map(|list| {
                list.iter()
                    .filter_map(socket_id)
                    .filter(|s| *s != sid)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn set_state(&self, channel_id: &str, sid: &str, mute: Value, deaf: Value, streaming: Value) -> bool {
        let mut states = self.states.lock().unwrap();
        let Some(list) = states.get_mut(channel_id) else {
            return false;
        };
        let mut changed = false;
        for u in list.iter_mut() {
            if socket_id(u) == Some(sid) {
                u["mute"] = mute.clone();
                u["deaf"] = deaf.clone();
                u["streaming"] = streaming.clone();
                changed = true;
            }
        }
        changed
    }

    /// Merges profile fields into every voice entry for this socket.
    pub fn update_profile(&self, sid: &str, patch: &Value) -> bool {
        let mut states = self.states.lock().unwrap();
        let mut changed = false;
        for list in states.values_mut() {
            for u in list.iter_mut() {
                if socket_id(u) == Some(sid) {
                    for key in PROFILE_KEYS {
                        if let Some(v) = patch.get(*key) {
                            u[*key] = v.clone();
                        }
                    }
                    changed = true;
                }
            }
        }
        changed
    }

    /// Snapshot of the whole map for `voice_state_sync`.
    pub fn snapshot(&self) -> Value {
        let states = self.states.lock().unwrap();
        serde_json::to_value(&*states).unwrap()
    }

    pub fn media_get(&self, channel_id: &str) -> Option<Value> {
        self.media.lock().unwrap().get(channel_id).cloned()
    }

    pub fn media_set(&self, channel_id: &str, media: Value) {
        self.media.lock().unwrap().insert(channel_id.to_string(), media);
    }

    pub fn media_remove(&self, channel_id: &str) {
        self.media.lock().unwrap().remove(channel_id);
    }

    /// Updates just the now-playing label/icon, creating a default entry if
    /// none exists (matching bot_now_playing in index.js).
    pub fn media_update_bar(&self, channel_id: &str, sid: &str, label: Option<Value>, icon: Option<Value>) -> Value {
        let mut media = self.media.lock().unwrap();
        let entry = media.entry(channel_id.to_string()).or_insert_with(|| {
            json!({
                "url": Value::Null, "title": Value::Null, "startTime": now_ms(),
                "volume": 0.5, "isYouTube": false, "youtubeUrl": "", "socketId": sid,
                "label": Value::Null, "icon": Value::Null
            })
        });
        if let Some(l) = label {
            entry["label"] = l;
        }
        if let Some(i) = icon {
            entry["icon"] = i;
        }
        entry.clone()
    }

    pub fn media_set_volume(&self, channel_id: &str, volume: Value) {
        if let Some(entry) = self.media.lock().unwrap().get_mut(channel_id) {
            entry["volume"] = volume;
        }
    }
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
