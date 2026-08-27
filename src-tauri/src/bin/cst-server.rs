use codex_switch_terminal_lib::server;

fn main() {
    // `cst-server --version` : self-check d'artefact (l'updater compare cette
    // valeur a la version attendue) et sonde de secours pour l'orchestrateur.
    // Doit fonctionner sans CST_ADMIN_TOKEN, donc avant toute initialisation.
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments
        .iter()
        .any(|arg| arg == "--version" || arg == "-V")
    {
        println!("cst-server {} ({})", server::VERSION, server::COMMIT);
        return;
    }

    if arguments
        .first()
        .is_some_and(|argument| argument == "device")
    {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                eprintln!("initialisation du runtime serveur impossible: {error}");
                std::process::exit(1);
            }
        };
        if let Err(error) = runtime.block_on(server::run_device_terminal_cli(&arguments[1..])) {
            eprintln!("{error}");
            std::process::exit(2);
        }
        return;
    }

    let config = match server::prepare_from_env_before_runtime() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("initialisation du runtime serveur impossible: {error}");
            std::process::exit(1);
        }
    };

    if let Err(error) = runtime.block_on(server::run(config)) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
