//! A test plugin that asks for ever more memory, so the host's memory limit has to stop it.

use serde_json::Value;
use solidcraft_plugin::{Host, Manifest, Plugin, Schema};

pub struct Hog;

impl Plugin for Hog {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: "test-hog".into(),
            name: "Test Hog".into(),
            version: "1.0.0".into(),
            api_version: solidcraft_plugin::API_VERSION,
            description: String::new(),
            display: Default::default(),
            fallback: Default::default(),
            preview: false,
        }
    }
    fn schema(&self) -> Schema {
        Schema::default()
    }
    fn evaluate(&self, _p: &Value, _ctx: &Value, _host: &mut dyn Host) -> Result<Value, String> {
        let mut kept: Vec<Vec<u8>> = Vec::new();
        loop {
            kept.push(std::hint::black_box(vec![1u8; 16 << 20]));
        }
    }
}

solidcraft_plugin_guest::export_plugin!(Hog);
