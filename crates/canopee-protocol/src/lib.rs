use canopee_identity::IdentityId;
use canopee_storage::{ExportBundle, Object, ObjectId, ObjectInfo};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub enum NodeCommand {
    Put { data: Vec<u8> },
    Get { id: ObjectId },
    List,
    Export { id: ObjectId },
    Import { bundle: ExportBundle },
    Status,
    Identity,
    Shutdown,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum NodeResponse {
    ObjectCreated { id: ObjectId },
    Object { object: Object },
    Objects { objects: Vec<ObjectInfo> },
    Exported { bundle: ExportBundle },
    Imported,
    Status { identity: String, objects: usize },
    Error { message: String },
    Identity { identity_id: IdentityId },
    ShutdownAccepted,
}
