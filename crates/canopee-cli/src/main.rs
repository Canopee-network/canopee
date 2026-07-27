use canopee_runtime::Runtime;
use canopee_storage::{ExportBundle, ObjectId, Verify};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "canopee")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Init,
    Identity,
    List,
    Get { id: String },
    Put { path: String },
    Export { id: String },
    Import { path: String },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init => {
            let Runtime {
                storage, identity, ..
            } = Runtime::open().await.unwrap();
            println!("Canopee identity created:\n{:?}", identity.identity_id);
            println!("Storage root:\n{:?}", storage.root())
        }

        Commands::Identity => {
            let runtime = Runtime::open().await.unwrap();
            println!("{:?}", runtime.identity.identity_id);
        }

        Commands::Put { path } => {
            let runtime = Runtime::open().await.unwrap();
            let bytes = tokio::fs::read(path).await.unwrap();
            let id = runtime.put(bytes).await.unwrap();
            println!("{:?}", id);
        }

        Commands::Get { id } => {
            let runtime = Runtime::open().await.unwrap();
            let object_id = ObjectId::new(&id);
            let object = runtime.get(&object_id).await.unwrap();

            println!("{:#?}", &object.payload);
            println!("{}", String::from_utf8_lossy(&object.payload.data));
        }

        Commands::List => {
            let runtime = Runtime::open().await.unwrap();
            let objects = runtime.list().await.unwrap();
            println!("Canopee Objects:\n");
            for object in objects {
                let valid = object.verify();
                println!("{}", object.id);
                println!("Owner: {:?}", object.payload.owner);
                println!("Size: {} bytes", object.payload.metadata.size);
                println!("Signature: {}", if valid { "✓ valid" } else { "✗ invalid" });
                println!();
            }
        }

        Commands::Export { id } => {
            let runtime = Runtime::open().await.unwrap();
            let object_id = ObjectId::new(&id);
            runtime.export_to_file(&object_id).await.unwrap();
        }

        Commands::Import { path } => {
            let runtime = Runtime::open().await.unwrap();
            let bytes = tokio::fs::read(path).await.unwrap();
            let export_bundle: ExportBundle = bincode::deserialize(&bytes).unwrap();
            runtime.import(export_bundle).await.unwrap();
        }
    }
}
