//! `chrome.storage.local` for the client in the TV app: one JSON object in a file in the app's
//! private storage, written atomically on every change (the client stores a few small values:
//! snapshots go to sessionStorage, never here).

use serde_json::{Map, Value};
use std::path::PathBuf;

pub struct Store {
    path: PathBuf,
    data: Map<String, Value>,
}

impl Store {
    pub fn open(path: PathBuf) -> Store {
        let data = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        Store { path, data }
    }

    fn keys(arg: &Value) -> Option<Vec<String>> {
        match arg {
            Value::Null => None,
            Value::String(s) => Some(vec![s.clone()]),
            Value::Array(a) => Some(a.iter().filter_map(|k| k.as_str().map(String::from)).collect()),
            Value::Object(o) => Some(o.keys().cloned().collect()),
            _ => Some(Vec::new()),
        }
    }

    /// `get(keys)`: `null` = everything; an object's values are defaults.
    pub fn get(&self, arg: &Value) -> Value {
        let mut out = match arg {
            Value::Object(o) => o.clone(),
            _ => Map::new(),
        };
        match Self::keys(arg) {
            None => out = self.data.clone(),
            Some(ks) => {
                for k in ks {
                    if let Some(v) = self.data.get(&k) {
                        out.insert(k, v.clone());
                    }
                }
            }
        }
        Value::Object(out)
    }

    pub fn set(&mut self, items: &Value) -> Result<(), String> {
        let o = items.as_object().ok_or("set: not an object")?;
        for (k, v) in o {
            self.data.insert(k.clone(), v.clone());
        }
        self.save()
    }

    pub fn remove(&mut self, arg: &Value) -> Result<(), String> {
        for k in Self::keys(arg).unwrap_or_default() {
            self.data.remove(&k);
        }
        self.save()
    }

    fn save(&self) -> Result<(), String> {
        crate::write_atomic(&self.path, Value::Object(self.data.clone()).to_string().as_bytes()).map_err(|e| e.to_string())
    }
}
