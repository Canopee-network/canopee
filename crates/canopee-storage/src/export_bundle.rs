use crate::object::Object;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct ExportBundle {
    pub version: u32,
    pub object: Object,
}

impl ExportBundle {
    pub fn new(&self, object: Object) -> Self {
        Self { version: 1, object }
    }
}
