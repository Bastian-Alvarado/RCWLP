//! Persistent host identity and paired phones, kept in %APPDATA%\RCWLP\host.json.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::protocol::Id;

#[derive(Serialize, Deserialize, Clone)]
pub struct PairedClient {
    #[serde(with = "hex")]
    pub id: Id,
    pub name: String,
    #[serde(with = "hex")]
    pub key: [u8; 32],
}

#[derive(Serialize, Deserialize)]
pub struct Store {
    #[serde(with = "hex")]
    pub host_id: Id,
    pub clients: Vec<PairedClient>,
    /// None for a store that only lives in memory (tests).
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl Store {
    pub fn load() -> Store {
        let dir = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("RCWLP");
        let path = dir.join("host.json");
        let loaded = fs::read_to_string(&path).ok().and_then(|s| {
            serde_json::from_str::<Store>(&s)
                .inspect_err(|e| eprintln!("Ignoring unreadable {}: {e}", path.display()))
                .ok()
        });
        match loaded {
            Some(mut s) => {
                s.path = Some(path);
                s
            }
            None => {
                let s = Store { host_id: crate::crypto::random(), clients: Vec::new(), path: Some(path) };
                s.save();
                s
            }
        }
    }

    /// A store that is never written to disk.
    pub fn in_memory(host_id: Id) -> Store {
        Store { host_id, clients: Vec::new(), path: None }
    }

    pub fn save(&self) {
        let Some(path) = &self.path else { return };
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("json.tmp");
            fs::write(&tmp, serde_json::to_string_pretty(self).unwrap())?;
            fs::rename(&tmp, path)
        };
        if let Err(e) = write() {
            eprintln!("Could not save {}: {e}", path.display());
        }
    }

    pub fn find(&self, id: &Id) -> Option<&PairedClient> {
        self.clients.iter().find(|c| &c.id == id)
    }

    pub fn upsert(&mut self, client: PairedClient) {
        self.clients.retain(|c| c.id != client.id);
        self.clients.push(client);
        self.save();
    }

    pub fn remove(&mut self, index: usize) -> Option<PairedClient> {
        (index < self.clients.len()).then(|| {
            let c = self.clients.remove(index);
            self.save();
            c
        })
    }
}

mod hex {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer, const N: usize>(v: &[u8; N], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&v.iter().map(|b| format!("{b:02x}")).collect::<String>())
    }

    pub fn deserialize<'de, D: Deserializer<'de>, const N: usize>(d: D) -> Result<[u8; N], D::Error> {
        let s = String::deserialize(d)?;
        if s.len() != N * 2 || !s.is_ascii() {
            return Err(D::Error::custom("bad hex length"));
        }
        let mut out = [0u8; N];
        for (i, b) in out.iter_mut().enumerate() {
            *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(D::Error::custom)?;
        }
        Ok(out)
    }
}
