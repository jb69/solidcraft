//! A test plugin that never finishes: its `evaluate` loops forever, so the host's fuel limit has
//! to stop it.

use serde_json::Value;
use solidcraft_plugin::{Host, Manifest, Plugin, Schema};

pub struct Spin;

impl Plugin for Spin {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: "test-spin".into(),
            name: "Test Spin".into(),
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
        let mut x = 1u64;
        loop {
            x = std::hint::black_box(x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1));
        }
    }
}

solidcraft_plugin_guest::export_plugin!(Spin);
