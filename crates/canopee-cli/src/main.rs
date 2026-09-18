use clap::{Parser, Subcommand};
mod app;
mod commands;
mod uri;

#[derive(Parser)]
#[command(name = "canopee")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
    /// Show content-addressed object ids (hidden by default — this CLI is
    /// designed so storage, sharing and fetching work by human names alone).
    #[arg(long, global = true)]
    ids: bool,
}

#[derive(Subcommand)]
enum Commands {
    Init,
    Identity,
    /// Prints this machine's device `PeerId` (from its per-device key) and
    /// the human-friendly device name it registers under its identity.
    Device,
    /// Lists the devices currently carrying this node's identity, as recorded
    /// in its `(owner, "devices")` list.
    Devices,
    /// Shows or edits this node's profile (display name).
    Profile {
        /// Set the display name. Omit to show the current profile.
        #[arg(long)]
        name: Option<String>,
    },
    List,
    Start,
    Get {
        /// The object's 64-char hex id, or a name you stored it under.
        id: String,
        /// Write the object's raw bytes to this file. Omit to print the
        /// content to stdout (as UTF-8 text when possible).
        #[arg(long)]
        output: Option<String>,
    },
    /// Shows an object's metadata (owner, type, size, name) plus a preview
    /// of its contents, so you can tell what a stored name actually is.
    Desc {
        /// The object's 64-char hex id, or a name you stored it under.
        id: String,
    },
    Put {
        path: String,
    },
    Export {
        id: String,
    },
    Import {
        path: String,
    },
    Status,
    Stop,
    Dial {
        addr: String,
    },
    ListenViaRelay {
        relay_addr: String,
    },
    Peers,
    RelayStatus,
    /// Announces on the DHT that this node provides the given object, so
    /// other peers can discover it via `find-providers`.
    Announce {
        /// The object's 64-char hex id, or a name you stored it under.
        id: String,
    },
    /// Lists peer ids that have announced themselves as providers of the
    /// given object on the DHT.
    FindProviders {
        /// The object's 64-char hex id, or a name you stored it under.
        id: String,
    },
    /// Fetches an object from a specific peer and imports it locally. Pass a
    /// raw id to fetch it directly, or a human name to resolve the peer's
    /// shared entry by name (`canopee fetch <peer> <name>`).
    Fetch {
        /// The peer id, identity (`canopee://identity/...`), or username to
        /// fetch from.
        peer_id: String,
        /// The object's 64-char hex id, or the name the peer shared it under.
        id: String,
    },
    /// Shares a stored object under a name: upserts a `shared: true` entry in
    /// your home index, publishes a `(owner, "entry:<name>")` pointer so other
    /// peers can fetch it by name, and announces the object on the DHT.
    Share {
        /// The name to share the object under (what others fetch it by).
        name: String,
        /// The object to share: a 64-char hex id, or a name you stored it
        /// under. Omit to share the local object already named `<name>`.
        id: Option<String>,
    },
    /// Stops sharing the home entry `<name>`: the object is withdrawn from
    /// the DHT and no longer served to peers.
    Unshare {
        name: String,
    },
    /// Lists the entries in your home index (name, object id, shared flag).
    Home,
    Publish {
        topic: String,
        message: String,
    },
    Chat {
        topic: String,
    },
    //canopee-cli app-manifest ./portfolio --name alice-portfolio
    AppManifest {
        directory_path: String,
        name: String,
    },
    AppInfo {
        id: String,
    },
    /// Opens a stored object with the system's default app for it. Give a
    /// file name you stored with `put` (or a raw id): the bytes are written
    /// to a temp file and handed to your OS opener, so `.mp3`, `.jpg`, `.pdf`
    /// etc. open in the right program. App manifests (<manifest-id>, or
    /// `--owner`/`--name` to resolve by name) are fetched and served over
    /// HTTP instead, as before.
    Open {
        id: Option<String>,
        #[arg(long)]
        owner: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        peer: Option<String>,
        #[arg(long, default_value_t = 0)]
        port: u16,
        /// Open the served URL in the default browser instead of just printing it.
        #[arg(long)]
        open: bool,
    },
    /// Resolves a `canopee://` URI (e.g. `canopee://alice/portfolio`) to an
    /// app and serves it in the default browser. Used by the OS-level URI
    /// scheme handler registered via `canopee uri-register`.
    Handle {
        uri: String,
        #[arg(long, default_value_t = 0)]
        port: u16,
    },
    /// Manages friendly short names (e.g. `alice`) that resolve to canonical
    /// `canopee://identity/<peer-id>` owners inside `canopee://` URIs.
    Alias {
        #[command(subcommand)]
        command: AliasCommand,
    },
    /// Registers this machine's OS to route `canopee://` URIs to `canopee handle`.
    UriRegister,
    /// Removes the OS-level `canopee://` scheme registration.
    UriUnregister,
    /// Claims and looks up globally unique usernames. A claimed username lets
    /// other peers discover and address you by name instead of a raw peer id
    /// (e.g. `canopee fetch <name> <id>`).
    Username {
        #[command(subcommand)]
        command: UsernameCommand,
    },
    /// Issues, lists, and revokes signed capabilities — "who is allowed to do
    /// what". A capability grants a subject (identity) a permission over a
    /// resource, signed so any peer can verify it against the issuer without
    /// a central authorization server.
    Cap {
        #[command(subcommand)]
        command: CapCommand,
    },
    /// Exposes the node to a browser tab as a local WebSocket gateway. Prints
    /// the demo URL (open this in a browser), then serves `http://127.0.0.1:
    /// <port>/` (demo page) and `ws://127.0.0.1:<port>/?token=…` (the JSON
    /// WebSocket bridge `crates/canopee-gateway/www/client.js` speaks).
    ///
    /// Access is restricted to the same machine: loopback bound, session-token
    /// gated, non-loopback `Origin`s rejected. The demo page injects the token
    /// itself, so visiting the printed URL gives a working bridge with nothing
    /// to configure. See `docs/gateway-tutorial.md`.
    Gateway {
        #[arg(long)]
        port: Option<u16>,
    },
    /// Exports the identity key as an encrypted file for transfer to another
    /// device (`canopee export-identity --passphrase … [--output path]`). The
    /// default output path is `identity-export.bin`.
    ExportIdentity {
        #[arg(long)]
        passphrase: String,
        #[arg(long, default_value = "identity-export.bin")]
        output: String,
    },
    /// Imports an identity key previously exported via `export-identity`
    /// (`canopee import-identity <path> --passphrase … [--overwrite]`). The
    /// imported key is written to disk; restart the node to adopt it.
    ImportIdentity {
        /// Path to the exported key file.
        path: String,
        #[arg(long)]
        passphrase: String,
        /// Overwrite the existing identity (the old key is backed up before
        /// replacement).
        #[arg(long)]
        overwrite: bool,
    },
    /// Pairs this node with another device on the same LAN so they share one
    /// identity.
    ///
    /// Run `canopee pair` (no arguments) on the NEW device: it prints a
    /// 12-character pairing code and a QR payload. On the device that already
    /// carries the identity, run:
    /// `canopee pair <qr-payload> --code <12-char-code>`
    /// where `<code>` is the code the new device displayed — typing it is the
    /// explicit approval; the code itself never travels over the wire.
    Pair {
        /// The QR payload printed by `canopee pair` on the new device, as a
        /// base64 blob. Omit to run the new-device side (print a code).
        qr: Option<String>,
        /// The 12-character pairing code shown on the new device. If omitted,
        /// you are prompted for it.
        #[arg(long)]
        code: Option<String>,
    },
    /// Refreshes this node's user records (profile, contacts, devices) from
    /// the network. With no argument, syncs from every registered device;
    /// with a peer id, syncs from that specific peer.
    Sync {
        /// The peer id to sync from. Omit to sync from all devices.
        peer_id: Option<String>,
    },
}

#[derive(Subcommand)]
enum UsernameCommand {
    /// Claims `<username>` for this node's identity so peers can discover it
    /// by name.
    Claim {
        username: String,
    },
    /// Shows the username currently claimed by this node's identity.
    Show,
    /// Reverse-resolves `<username>` to its canonical owner identity.
    Lookup {
        username: String,
    },
}

#[derive(Subcommand)]
enum CapCommand {
    /// Grants `<subject>` a permission over `<resource>`.
    ///
    /// `<subject>` is a `canopee://identity/...` reference.
    /// `<resource>` is `object:<id>`, `channel:<topic>`, or `shared:<owner>:<name>`.
    /// At least one of `--read`, `--write`, `--publish` must be given.
    /// By default the grant never expires; pass `--expires <unix-seconds>`.
    Grant {
        subject: String,
        resource: String,
        #[arg(long)]
        read: bool,
        #[arg(long)]
        write: bool,
        #[arg(long)]
        publish: bool,
        #[arg(long)]
        expires: Option<u64>,
    },
    /// Lists every capability this node's identity has issued, with its
    /// revocation state.
    List,
    /// Revokes a previously issued capability by its (content-derived) id.
    Revoke {
        id: String,
    },
    /// Verifies a capability presented out of band. Pass a base64 `ExportCapability`
    /// bundle, or `--subject <identity>` to check whether the subject currently
    /// holds an unrevoked, unexpired grant of `<permission>` on `<resource>`
    /// from this node's identity.
    Check {
        /// A base64-encoded capability bundle to verify end to end.
        bundle: Option<String>,
        #[arg(long)]
        subject: Option<String>,
        #[arg(long)]
        permission: Option<String>,
        #[arg(long)]
        resource: Option<String>,
    },
}

#[derive(Subcommand)]
enum AliasCommand {
    /// Maps `<name>` to a canonical owner (`canopee://identity/<peer-id>`).
    Set {
        name: String,
        owner: String,
    },
    /// Lists all known aliases.
    List,
    /// Removes `<name>`.
    #[command(alias = "rm")]
    Remove {
        name: String,
    },
}

#[tokio::main]
async fn main() {
    let Cli { command, ids } = Cli::parse();

    match command {
        Commands::Init => commands::daemon::init().await,
        Commands::Identity => commands::identity::identity().await,
        Commands::Device => commands::identity::device().await,
        Commands::Devices => commands::identity::devices().await,
        Commands::Profile { name } => commands::identity::profile(name).await,
        Commands::List => commands::objects::list(ids).await,
        Commands::Start => commands::daemon::start().await,
        Commands::Get { id, output } => commands::objects::get(id, output).await,
        Commands::Desc { id } => commands::objects::desc(id, ids).await,
        Commands::Put { path } => commands::objects::put(path, ids).await,
        Commands::Export { id } => commands::objects::export(id).await,
        Commands::Import { path } => commands::objects::import(path).await,
        Commands::Status => commands::daemon::status().await,
        Commands::Stop => commands::daemon::stop().await,
        Commands::Dial { addr } => commands::network::dial(addr).await,
        Commands::ListenViaRelay { relay_addr } => {
            commands::network::listen_via_relay(relay_addr).await
        }
        Commands::Peers => commands::network::peers().await,
        Commands::RelayStatus => commands::network::relay_status().await,
        Commands::Announce { id } => commands::network::announce(id, ids).await,
        Commands::FindProviders { id } => commands::network::find_providers(id).await,
        Commands::Fetch { peer_id, id } => commands::network::fetch(peer_id, id, ids).await,
        Commands::Share { name, id } => commands::sharing::share(name, id, ids).await,
        Commands::Unshare { name } => commands::sharing::unshare(name).await,
        Commands::Home => commands::sharing::home(ids).await,
        Commands::Publish { topic, message } => commands::network::publish(topic, message).await,
        Commands::Chat { topic } => commands::network::chat(topic).await,
        Commands::AppManifest { directory_path, name } => {
            commands::apps::app_manifest(directory_path, name).await
        }
        Commands::AppInfo { id } => commands::apps::app_info(id).await,
        Commands::Open {
            id,
            owner,
            name,
            peer,
            port,
            open,
        } => commands::apps::open(id, owner, name, peer, port, open).await,
        Commands::Handle { uri, port } => commands::apps::handle(uri, port).await,
        Commands::Alias { command } => commands::aliases::alias(command).await,
        Commands::UriRegister => commands::aliases::uri_register().await,
        Commands::UriUnregister => commands::aliases::uri_unregister().await,
        Commands::Username { command } => commands::identity::username(command).await,
        Commands::Cap { command } => commands::capabilities::capabilities(command).await,
        Commands::Gateway { port } => commands::gateway::gateway(port).await,
        Commands::ExportIdentity { passphrase, output } => {
            commands::identity::export_identity(passphrase, output).await
        }
        Commands::ImportIdentity {
            path,
            passphrase,
            overwrite,
        } => commands::identity::import_identity(path, passphrase, overwrite).await,
        Commands::Pair { qr, code } => commands::pairing::pair(qr, code).await,
        Commands::Sync { peer_id } => commands::sharing::sync(peer_id).await,
    }
}