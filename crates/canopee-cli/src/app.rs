use canopee_sdk::{CanopeeClient, IdentityId};
use canopee_storage::ObjectId;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::Path};

#[derive(Debug, Serialize, Deserialize)]
pub struct AppManifest {
    pub name: String,
    pub owner: IdentityId,
    pub entrypoint: ObjectId,
    pub assets: HashMap<String, ObjectId>,
}

pub async fn publish_directory(
    client: &CanopeeClient,
    directory: &Path,
) -> anyhow::Result<(ObjectId, HashMap<String, ObjectId>)> {
    let mut assets = HashMap::new();
    let mut entrypoint = None;

    for entry in walkdir::WalkDir::new(directory) {
        let entry = entry?;
        if entry.file_type().is_dir() {
            continue;
        }
        let path = entry.path();
        let data = tokio::fs::read(path).await.unwrap();
        let object_id = client.put_file(data).await?;
        let relative = path.strip_prefix(directory)?.to_string_lossy().to_string();

        println!("{} -> {}", relative, object_id);

        if relative == "index.html" {
            entrypoint = Some(object_id.clone());
        } else {
            assets.insert(format!("/{}", relative), object_id);
        }
    }
    let entrypoint = entrypoint.ok_or(anyhow::anyhow!("portfolio has no index.html"))?;
    Ok((entrypoint, assets))
}
