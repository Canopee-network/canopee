use crate::uri;

pub(crate) async fn alias(command: crate::AliasCommand) {
    match command {
        crate::AliasCommand::Set { name, owner } => match uri::set_alias(&name, &owner) {
            Ok(()) => println!("alias \"{name}\" -> {owner}"),
            Err(e) => eprintln!("Error: {e:#}"),
        },
        crate::AliasCommand::List => match uri::list_aliases() {
            Ok(aliases) => {
                if aliases.is_empty() {
                    println!("No aliases set");
                }
                for (name, owner) in aliases {
                    println!("{name} -> {owner}");
                }
            }
            Err(e) => eprintln!("Error: {e:#}"),
        },
        crate::AliasCommand::Remove { name } => match uri::remove_alias(&name) {
            Ok(true) => println!("Removed alias \"{name}\""),
            Ok(false) => println!("No alias \"{name}\" found"),
            Err(e) => eprintln!("Error: {e:#}"),
        },
    }
}

pub(crate) async fn uri_register() {
    match uri::register_scheme() {
        Ok(()) => println!("Registered canopee:// scheme handler"),
        Err(e) => {
            eprintln!("Error: {e:#}");
            std::process::exit(1);
        }
    }
}

pub(crate) async fn uri_unregister() {
    match uri::unregister_scheme() {
        Ok(()) => println!("Removed canopee:// scheme handler"),
        Err(e) => {
            eprintln!("Error: {e:#}");
            std::process::exit(1);
        }
    }
}
